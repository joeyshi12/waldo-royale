//! Waldo Royale lobby server: serves the static client and runs the
//! multiplayer protocol over `/ws`. State lives in memory; scoring is
//! authoritative via waldo-core.

use std::{
    collections::HashMap,
    net::SocketAddr,
    path::PathBuf,
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc, Mutex,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::{
    extract::{
        ws::{Message, WebSocket},
        State, WebSocketUpgrade,
    },
    response::Response,
    routing::get,
    Router,
};
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use tower_http::{services::ServeDir, set_header::SetResponseHeaderLayer};
use waldo_core::{
    bonus_points,
    math::v3,
    mutator_opts,
    protocol::{ClientMsg, Config, PlayerInfo, RoundEntry, ServerMsg, Standing},
    rank_points, rng::Rng, score_round, WALDO_HIT_RADIUS,
};

const DEFAULT_PORT: u16 = 8017;
const DEFAULT_INTERMISSION_MS: u64 = 8000;

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64
}

fn intermission_ms() -> u64 {
    std::env::var("WALDO_INTERMISSION_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_INTERMISSION_MS)
}

#[derive(PartialEq, Clone, Copy)]
enum Phase {
    Waiting,
    Playing,
    Intermission,
}

struct Player {
    id: u32,
    name: String,
    tx: UnboundedSender<String>,
    /// Private session token for reconnecting.
    token: String,
    connected: bool,
    /// Bumped on reconnect to invalidate pending cleanup timers.
    disconnect_gen: u64,
    /// Finish order for Waldo this round (1-based), 0 = not yet.
    found_rank: u32,
    /// ms into the round when Waldo was clicked.
    found_at_ms: i64,
    misses: u32,
    wenda: bool,
    woof: bool,
    wizard: bool,
    odlaw: bool,
    total: u32,
}

impl Player {
    fn round_score(&self) -> u32 {
        score_round(self.found_rank, self.misses, self.wenda, self.woof, self.wizard, self.odlaw)
    }
}

struct Lobby {
    code: String,
    players: Vec<Player>,
    config: Config,
    phase: Phase,
    round: u32,
    seed: u32,
    round_ends_ms: u64,
    /// Waldo's position for the current round, cached at round start.
    waldo: [f32; 3],
    /// Supporting cast positions for the current round: (role, pos).
    cast: Vec<(String, [f32; 3])>,
    /// Current round's mutator ("" = none).
    mutator: String,
    /// Actual round length in ms (mutators may shorten it).
    round_len_ms: u64,
    /// Bumped whenever a scheduled timer becomes stale.
    timer_gen: u64,
}

struct App {
    lobbies: Mutex<HashMap<String, Lobby>>,
    next_id: AtomicU32,
}

type Shared = Arc<App>;

fn send_to(p: &Player, msg: &ServerMsg) {
    let _ = p.tx.send(serde_json::to_string(msg).unwrap());
}

fn broadcast(lobby: &Lobby, msg: &ServerMsg) {
    let s = serde_json::to_string(msg).unwrap();
    for p in &lobby.players {
        let _ = p.tx.send(s.clone());
    }
}

fn lobby_snapshot(lobby: &Lobby, you: u32, token: &str) -> ServerMsg {
    ServerMsg::Lobby {
        code: lobby.code.clone(),
        you,
        token: token.to_string(),
        players: lobby
            .players
            .iter()
            .enumerate()
            .map(|(i, p)| PlayerInfo {
                id: p.id,
                name: p.name.clone(),
                is_host: i == 0,
                connected: p.connected,
            })
            .collect(),
        config: lobby.config,
    }
}

/// Send each player a Lobby message with their own id and session token.
fn broadcast_lobby(lobby: &Lobby) {
    for p in &lobby.players {
        send_to(p, &lobby_snapshot(lobby, p.id, &p.token));
    }
}

fn gen_token() -> String {
    let t = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    let mut r = Rng::new(t.as_nanos() as u64 ^ (t.subsec_nanos() as u64) << 17);
    format!("{:08x}{:08x}{:08x}", r.next_u32(), r.next_u32(), r.next_u32())
}

fn grace_ms() -> u64 {
    std::env::var("WALDO_GRACE_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(60_000)
}

fn leaderboard(lobby: &Lobby) -> Vec<Standing> {
    let mut rows: Vec<Standing> = lobby
        .players
        .iter()
        .map(|p| Standing { id: p.id, name: p.name.clone(), total: p.total })
        .collect();
    rows.sort_by(|a, b| b.total.cmp(&a.total));
    rows
}

fn start_round(state: &Shared, lobby: &mut Lobby) {
    lobby.round += 1;
    lobby.phase = Phase::Playing;
    lobby.seed = {
        let t = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().subsec_nanos();
        let mut r = Rng::new(t as u64 ^ (now_ms() << 16) ^ lobby.round as u64);
        r.next_u32()
    };
    lobby.mutator = std::env::var("WALDO_MUTATOR").unwrap_or_else(|_| {
        let t = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().subsec_nanos();
        let mut r = Rng::new(t as u64 ^ lobby.seed as u64);
        match r.below(9) {
            0 => "night",
            1 => "lightning",
            2 => "crowded",
            3 => "tiny",
            _ => "",
        }
        .to_string()
    });
    let secs = if lobby.mutator == "lightning" {
        (lobby.config.round_secs / 3).clamp(15, 45)
    } else {
        lobby.config.round_secs
    };
    lobby.round_len_ms = secs as u64 * 1000;
    lobby.round_ends_ms = now_ms() + lobby.round_len_ms;
    let world = waldo_core::generate_world_opts(lobby.seed, mutator_opts(&lobby.mutator));
    lobby.waldo = world.waldo.pos;
    lobby.cast = world.cast.iter().map(|c| (c.role.clone(), c.pos)).collect();
    for p in &mut lobby.players {
        p.found_rank = 0;
        p.found_at_ms = -1;
        p.misses = 0;
        p.wenda = false;
        p.woof = false;
        p.wizard = false;
        p.odlaw = false;
    }
    lobby.timer_gen += 1;
    broadcast(
        lobby,
        &ServerMsg::RoundStart {
            round: lobby.round,
            total_rounds: lobby.config.rounds,
            seed: lobby.seed,
            round_secs: secs,
            mutator: lobby.mutator.clone(),
            ends_at_ms: lobby.round_ends_ms,
        },
    );

    let gen = lobby.timer_gen;
    let code = lobby.code.clone();
    let state = state.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(secs as u64)).await;
        let mut lobbies = state.lobbies.lock().unwrap();
        if let Some(lobby) = lobbies.get_mut(&code) {
            if lobby.timer_gen == gen && lobby.phase == Phase::Playing {
                finish_round(&state, lobby);
            }
        }
    });
}

fn finish_round(state: &Shared, lobby: &mut Lobby) {
    lobby.phase = Phase::Intermission;
    lobby.timer_gen += 1; // invalidate the timeout timer

    let waldo = lobby.waldo;
    let mut results: Vec<RoundEntry> = lobby
        .players
        .iter_mut()
        .map(|p| {
            let score = p.round_score();
            p.total += score;
            RoundEntry {
                id: p.id,
                name: p.name.clone(),
                found: p.found_rank > 0,
                rank: p.found_rank,
                time_ms: p.found_at_ms,
                misses: p.misses,
                bonus: bonus_points(p.wenda, p.woof, p.wizard, p.odlaw),
                score,
                total: p.total,
            }
        })
        .collect();
    results.sort_by(|a, b| b.score.cmp(&a.score));

    let last = lobby.round >= lobby.config.rounds;
    let pause = intermission_ms();
    broadcast(
        lobby,
        &ServerMsg::RoundResult {
            round: lobby.round,
            waldo,
            results,
            next_in_ms: pause,
        },
    );

    let gen = lobby.timer_gen;
    let code = lobby.code.clone();
    let state_c = state.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(pause)).await;
        let mut lobbies = state_c.lobbies.lock().unwrap();
        if let Some(lobby) = lobbies.get_mut(&code) {
            if lobby.timer_gen == gen && lobby.phase == Phase::Intermission {
                if last {
                    broadcast(lobby, &ServerMsg::GameOver { leaderboard: leaderboard(lobby) });
                    lobby.phase = Phase::Waiting;
                    lobby.round = 0;
                    broadcast_lobby(lobby);
                } else {
                    start_round(&state_c, lobby);
                }
            }
        }
    });
}

fn gen_code(existing: &HashMap<String, Lobby>) -> String {
    let mut rng = Rng::new(
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().subsec_nanos() as u64 ^ now_ms(),
    );
    loop {
        let code: String =
            (0..4).map(|_| (b'A' + rng.below(26) as u8) as char).collect();
        if !existing.contains_key(&code) {
            return code;
        }
    }
}

fn err(tx: &UnboundedSender<String>, message: &str) {
    let _ = tx.send(
        serde_json::to_string(&ServerMsg::Error { message: message.into() }).unwrap(),
    );
}

fn sanitize_name(name: &str) -> String {
    let n: String = name.chars().filter(|c| !c.is_control()).take(20).collect();
    let n = n.trim().to_string();
    if n.is_empty() {
        "Player".into()
    } else {
        n
    }
}

fn handle_msg(
    state: &Shared,
    tx: &UnboundedSender<String>,
    session: &mut Option<(String, u32)>,
    raw: &str,
) {
    let Ok(msg) = serde_json::from_str::<ClientMsg>(raw) else {
        err(tx, "bad message");
        return;
    };
    let mut lobbies = state.lobbies.lock().unwrap();

    match msg {
        ClientMsg::Create { name } => {
            if session.is_some() {
                return err(tx, "already in a lobby");
            }
            let code = gen_code(&lobbies);
            let id = state.next_id.fetch_add(1, Ordering::Relaxed);
            let lobby = Lobby {
                code: code.clone(),
                players: vec![Player {
                    id,
                    name: sanitize_name(&name),
                    tx: tx.clone(),
                    token: gen_token(),
                    connected: true,
                    disconnect_gen: 0,
                    found_rank: 0,
                    found_at_ms: -1,
                    misses: 0,
                    wenda: false,
                    woof: false,
                    wizard: false,
                    odlaw: false,
                    total: 0,
                }],
                config: Config::default(),
                phase: Phase::Waiting,
                round: 0,
                seed: 0,
                round_ends_ms: 0,
                waldo: [0.0; 3],
                cast: Vec::new(),
                mutator: String::new(),
                round_len_ms: 0,
                timer_gen: 0,
            };
            broadcast_lobby(&lobby);
            lobbies.insert(code.clone(), lobby);
            *session = Some((code, id));
        }

        ClientMsg::Join { code, name } => {
            if session.is_some() {
                return err(tx, "already in a lobby");
            }
            let code = code.trim().to_uppercase();
            let Some(lobby) = lobbies.get_mut(&code) else {
                return err(tx, "lobby not found");
            };
            if lobby.phase != Phase::Waiting {
                return err(tx, "game already in progress");
            }
            if lobby.players.len() >= 12 {
                return err(tx, "lobby is full");
            }
            let id = state.next_id.fetch_add(1, Ordering::Relaxed);
            lobby.players.push(Player {
                id,
                name: sanitize_name(&name),
                tx: tx.clone(),
                token: gen_token(),
                connected: true,
                disconnect_gen: 0,
                found_rank: 0,
                found_at_ms: -1,
                misses: 0,
                wenda: false,
                woof: false,
                wizard: false,
                odlaw: false,
                total: 0,
            });
            broadcast_lobby(lobby);
            *session = Some((code, id));
        }

        ClientMsg::Rejoin { code, token } => {
            if session.is_some() {
                return err(tx, "already in a lobby");
            }
            let code = code.trim().to_uppercase();
            let Some(lobby) = lobbies.get_mut(&code) else {
                return err(tx, "session expired");
            };
            let Some(p) = lobby.players.iter_mut().find(|p| p.token == token) else {
                return err(tx, "session expired");
            };
            p.tx = tx.clone();
            p.connected = true;
            p.disconnect_gen += 1;
            let id = p.id;
            let restore = if lobby.phase == Phase::Playing {
                Some(ServerMsg::Restore {
                    round: lobby.round,
                    total_rounds: lobby.config.rounds,
                    seed: lobby.seed,
                    round_secs: (lobby.round_len_ms / 1000) as u32,
                    mutator: lobby.mutator.clone(),
                    ends_at_ms: lobby.round_ends_ms,
                    found_rank: p.found_rank,
                    misses: p.misses,
                    wenda: p.wenda,
                    woof: p.woof,
                    wizard: p.wizard,
                    odlaw: p.odlaw,
                })
            } else {
                None
            };
            broadcast_lobby(lobby);
            if let Some(r) = restore {
                let p = lobby.players.iter().find(|p| p.id == id).unwrap();
                send_to(p, &r);
            }
            *session = Some((code, id));
        }

        ClientMsg::Configure { rounds, round_secs } => {
            let Some((code, id)) = session else { return err(tx, "not in a lobby") };
            let Some(lobby) = lobbies.get_mut(code) else { return };
            if lobby.players.first().map(|p| p.id) != Some(*id) {
                return err(tx, "only the host can change settings");
            }
            if lobby.phase != Phase::Waiting {
                return err(tx, "cannot change settings mid-game");
            }
            lobby.config = Config { rounds, round_secs }.clamped();
            broadcast_lobby(lobby);
        }

        ClientMsg::Start => {
            let Some((code, id)) = session else { return err(tx, "not in a lobby") };
            let Some(lobby) = lobbies.get_mut(code) else { return };
            if lobby.players.first().map(|p| p.id) != Some(*id) {
                return err(tx, "only the host can start");
            }
            if lobby.phase != Phase::Waiting {
                return err(tx, "game already running");
            }
            for p in &mut lobby.players {
                p.total = 0;
            }
            lobby.round = 0;
            start_round(state, lobby);
        }

        ClientMsg::Click { pos } => {
            let Some((code, id)) = session else { return };
            let Some(lobby) = lobbies.get_mut(code) else { return };
            if lobby.phase != Phase::Playing {
                return;
            }
            if !pos.iter().all(|c| c.is_finite()) {
                return;
            }
            let ends = lobby.round_ends_ms;
            let round_len_ms = lobby.round_len_ms;
            let total_ms = ends.saturating_sub(now_ms());
            let click = v3(pos[0], pos[1], pos[2]);
            let waldo = v3(lobby.waldo[0], lobby.waldo[1], lobby.waldo[2]);

            // resolve the click target: Waldo first, then the nearest cast
            // member in range, otherwise a miss
            let cast_hit: Option<String> = lobby
                .cast
                .iter()
                .map(|(role, p)| (role.clone(), click.distance(v3(p[0], p[1], p[2]))))
                .filter(|(_, d)| *d <= WALDO_HIT_RADIUS)
                .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
                .map(|(role, _)| role);
            let found_before = lobby.players.iter().filter(|p| p.found_rank > 0).count() as u32;
            let total_players = lobby.players.len() as u32;

            let Some(p) = lobby.players.iter_mut().find(|p| p.id == *id) else { return };
            let hit_waldo = click.distance(waldo) <= WALDO_HIT_RADIUS;

            let target: &str;
            let points: i32;
            if hit_waldo && p.found_rank == 0 {
                p.found_rank = found_before + 1;
                p.found_at_ms = (round_len_ms as i64).saturating_sub(total_ms as i64).max(0);
                target = "waldo";
                points = rank_points(p.found_rank) as i32;
            } else if hit_waldo {
                return; // already found him; ignore repeat clicks on Waldo
            } else if let Some(role) = cast_hit {
                let (already, flag): (bool, &mut bool) = match role.as_str() {
                    "wenda" => (p.wenda, &mut p.wenda),
                    "woof" => (p.woof, &mut p.woof),
                    "wizard" => (p.wizard, &mut p.wizard),
                    _ => (p.odlaw, &mut p.odlaw),
                };
                if already {
                    return; // each character interacts once per round
                }
                *flag = true;
                target = match role.as_str() {
                    "wenda" => "wenda",
                    "woof" => "woof",
                    "wizard" => "wizard",
                    _ => "odlaw",
                };
                points = match target {
                    "wenda" => waldo_core::scoring::BONUS_WENDA as i32,
                    "woof" => waldo_core::scoring::BONUS_WOOF as i32,
                    "wizard" => waldo_core::scoring::BONUS_WIZARD as i32,
                    _ => -(waldo_core::scoring::ODLAW_PENALTY as i32),
                };
            } else {
                p.misses += 1;
                target = "miss";
                points = -(waldo_core::scoring::MISS_PENALTY as i32);
            }

            let msg = ServerMsg::ClickResult {
                target: target.to_string(),
                points,
                round_score: p.round_score(),
                misses: p.misses,
            };
            send_to(p, &msg);

            if target == "waldo" {
                let found = found_before + 1;
                broadcast(lobby, &ServerMsg::PlayerFound { id: *id, found, total: total_players });
                if all_connected_found(lobby) {
                    finish_round(state, lobby);
                }
            }
        }

    }
}

/// Remove a player for real (grace period expired or lobby shutdown).
fn hard_remove(state: &Shared, lobbies: &mut HashMap<String, Lobby>, code: &str, id: u32) {
    let Some(lobby) = lobbies.get_mut(code) else { return };
    lobby.players.retain(|p| p.id != id);
    if lobby.players.is_empty() {
        lobbies.remove(code);
        return;
    }
    broadcast_lobby(lobby);
    if lobby.phase == Phase::Playing && all_connected_found(lobby) {
        finish_round(state, lobby);
    }
}

fn all_connected_found(lobby: &Lobby) -> bool {
    let connected: Vec<_> = lobby.players.iter().filter(|p| p.connected).collect();
    !connected.is_empty() && connected.iter().all(|p| p.found_rank > 0)
}

/// A socket dropped: keep the player for a grace period so they can rejoin.
fn leave(state: &Shared, code: &str, id: u32) {
    let mut lobbies = state.lobbies.lock().unwrap();
    let Some(lobby) = lobbies.get_mut(code) else { return };
    let Some(p) = lobby.players.iter_mut().find(|p| p.id == id) else { return };
    p.connected = false;
    p.disconnect_gen += 1;
    let gen = p.disconnect_gen;
    broadcast_lobby(lobby);
    if lobby.phase == Phase::Playing && all_connected_found(lobby) {
        finish_round(state, lobby);
    }

    let state = state.clone();
    let code = code.to_string();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(grace_ms())).await;
        let mut lobbies = state.lobbies.lock().unwrap();
        let still_gone = lobbies
            .get(&code)
            .and_then(|l| l.players.iter().find(|p| p.id == id))
            .map(|p| !p.connected && p.disconnect_gen == gen)
            .unwrap_or(false);
        if still_gone {
            hard_remove(&state, &mut lobbies, &code, id);
        }
    });
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<Shared>) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(mut socket: WebSocket, state: Shared) {
    let (tx, mut rx) = unbounded_channel::<String>();
    let mut session: Option<(String, u32)> = None;

    loop {
        tokio::select! {
            out = rx.recv() => match out {
                Some(s) => {
                    if socket.send(Message::Text(s.into())).await.is_err() {
                        break;
                    }
                }
                None => break,
            },
            inc = socket.recv() => match inc {
                Some(Ok(Message::Text(t))) => handle_msg(&state, &tx, &mut session, &t),
                Some(Ok(Message::Close(_))) | None => break,
                Some(Ok(_)) => {}
                Some(Err(_)) => break,
            },
        }
    }

    if let Some((code, id)) = session {
        leave(&state, &code, id);
    }
}

fn web_dir() -> PathBuf {
    if let Ok(d) = std::env::var("WALDO_WEB_DIR") {
        return PathBuf::from(d);
    }
    let candidates = [
        PathBuf::from("web"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../web"),
    ];
    candidates
        .into_iter()
        .find(|p| p.join("index.html").exists())
        .unwrap_or_else(|| PathBuf::from("web"))
}

#[tokio::main]
async fn main() {
    let state: Shared =
        Arc::new(App { lobbies: Mutex::new(HashMap::new()), next_id: AtomicU32::new(1) });

    let port: u16 = std::env::var("PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(DEFAULT_PORT);
    let dir = web_dir();

    let app = Router::new()
        .route("/ws", get(ws_handler))
        .fallback_service(ServeDir::new(&dir))
        // force CDN/browser revalidation so clients never run stale code
        .layer(SetResponseHeaderLayer::overriding(
            axum::http::header::CACHE_CONTROL,
            axum::http::HeaderValue::from_static("no-cache"),
        ))
        .with_state(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    println!("Waldo Royale server on http://localhost:{port} (serving {})", dir.display());
    let listener = tokio::net::TcpListener::bind(addr).await.expect("bind");
    axum::serve(listener, app).await.expect("serve");
}
