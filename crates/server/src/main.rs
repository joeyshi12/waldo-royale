//! Waldo Royale lobby server.
//!
//! Serves the static client from `web/` and runs the multiplayer protocol
//! over `/ws`. All game state lives in memory. Scoring is authoritative:
//! the server re-derives each round's world from the seed via `waldo-core`
//! and computes every player's score itself.

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
    math::v3,
    protocol::{ClientMsg, Config, PlayerInfo, RoundEntry, ServerMsg, Standing},
    rng::Rng,
    score_find, WALDO_HIT_RADIUS,
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
    found: bool,
    /// ms into the round when Waldo was clicked.
    found_at_ms: i64,
    /// fraction of round time remaining at the find (drives the score).
    found_frac: f32,
    misses: u32,
    total: u32,
}

struct Lobby {
    code: String,
    players: Vec<Player>,
    config: Config,
    phase: Phase,
    round: u32,
    seed: u32,
    round_ends_ms: u64,
    /// Waldo's position for the current round (cached at round start so
    /// clicks don't regenerate the world).
    waldo: [f32; 3],
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

fn lobby_snapshot(lobby: &Lobby, you: u32) -> ServerMsg {
    ServerMsg::Lobby {
        code: lobby.code.clone(),
        you,
        players: lobby
            .players
            .iter()
            .enumerate()
            .map(|(i, p)| PlayerInfo { id: p.id, name: p.name.clone(), is_host: i == 0 })
            .collect(),
        config: lobby.config,
    }
}

/// Send each player a Lobby message with their own `you` id.
fn broadcast_lobby(lobby: &Lobby) {
    for p in &lobby.players {
        send_to(p, &lobby_snapshot(lobby, p.id));
    }
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
        // unpredictable enough for a party game
        let t = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().subsec_nanos();
        let mut r = Rng::new(t as u64 ^ (now_ms() << 16) ^ lobby.round as u64);
        r.next_u32()
    };
    let secs = lobby.config.round_secs;
    lobby.round_ends_ms = now_ms() + secs as u64 * 1000;
    lobby.waldo = waldo_core::generate_world(lobby.seed).waldo.pos;
    for p in &mut lobby.players {
        p.found = false;
        p.found_at_ms = -1;
        p.found_frac = 0.0;
        p.misses = 0;
    }
    lobby.timer_gen += 1;
    broadcast(
        lobby,
        &ServerMsg::RoundStart {
            round: lobby.round,
            total_rounds: lobby.config.rounds,
            seed: lobby.seed,
            round_secs: secs,
            ends_at_ms: lobby.round_ends_ms,
        },
    );

    // schedule the round timeout
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
            let score = score_find(p.found, p.found_frac, p.misses);
            p.total += score;
            RoundEntry {
                id: p.id,
                name: p.name.clone(),
                found: p.found,
                time_ms: p.found_at_ms,
                misses: p.misses,
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

    // schedule next round / game over
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
                    found: false,
                    found_at_ms: -1,
                    found_frac: 0.0,
                    misses: 0,
                    total: 0,
                }],
                config: Config::default(),
                phase: Phase::Waiting,
                round: 0,
                seed: 0,
                round_ends_ms: 0,
                waldo: [0.0; 3],
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
                found: false,
                found_at_ms: -1,
                found_frac: 0.0,
                misses: 0,
                total: 0,
            });
            broadcast_lobby(lobby);
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
            let total_ms = lobby.config.round_secs as f32 * 1000.0;
            let waldo = v3(lobby.waldo[0], lobby.waldo[1], lobby.waldo[2]);
            let Some(p) = lobby.players.iter_mut().find(|p| p.id == *id) else { return };
            if p.found {
                return; // already done this round
            }
            let hit = v3(pos[0], pos[1], pos[2]).distance(waldo) <= WALDO_HIT_RADIUS;
            if hit {
                p.found = true;
                let remaining = ends.saturating_sub(now_ms()) as f32;
                p.found_frac = (remaining / total_ms).clamp(0.0, 1.0);
                p.found_at_ms = (total_ms - remaining) as i64;
                let score = score_find(true, p.found_frac, p.misses);
                let misses = p.misses;
                send_to(p, &ServerMsg::ClickResult { hit: true, score, misses });
                let found = lobby.players.iter().filter(|p| p.found).count() as u32;
                let total = lobby.players.len() as u32;
                broadcast(lobby, &ServerMsg::PlayerFound { id: *id, found, total });
                if found == total {
                    finish_round(state, lobby);
                }
            } else {
                p.misses += 1;
                let misses = p.misses;
                send_to(p, &ServerMsg::ClickResult { hit: false, score: 0, misses });
            }
        }
    }
}

fn leave(state: &Shared, code: &str, id: u32) {
    let mut lobbies = state.lobbies.lock().unwrap();
    let Some(lobby) = lobbies.get_mut(code) else { return };
    lobby.players.retain(|p| p.id != id);
    if lobby.players.is_empty() {
        lobbies.remove(code);
        return;
    }
    broadcast_lobby(lobby); // also announces the (possibly new) host
    if lobby.phase == Phase::Playing
        && lobby.players.iter().all(|p| p.found)
    {
        finish_round(state, lobby);
    }
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
    // works both from the workspace root and from an installed layout
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
        // game code changes often and is small: force CDN/browser revalidation
        // so players never run a stale client after a deploy
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
