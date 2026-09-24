//! Waldo Royale lobby server: serves the static client and carries the multiplayer
//! protocol over `/ws`.
//!
//! The game rules are not here. They live in `waldo_core::lobby` as a state machine
//! with no I/O; this binary is the shell around it that owns the sockets, the clock,
//! the timers and the id allocator, and translates between them.

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
    lobby::{Event, Lobby, Out, Settings, Timer},
    protocol::{ClientMsg, ServerMsg},
    rng::Rng,
};

const DEFAULT_PORT: u16 = 8017;

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64
}

fn env_u64(key: &str, fallback: u64) -> u64 {
    std::env::var(key).ok().and_then(|v| v.parse().ok()).unwrap_or(fallback)
}

fn settings() -> Settings {
    let d = Settings::default();
    Settings {
        intermission_ms: env_u64("WALDO_INTERMISSION_MS", d.intermission_ms),
        grace_ms: env_u64("WALDO_GRACE_MS", d.grace_ms),
        mutator: std::env::var("WALDO_MUTATOR").ok(),
        max_players: d.max_players,
    }
}

/// One lobby plus the sockets to reach its players by.
struct Room {
    lobby: Lobby,
    sockets: HashMap<u32, UnboundedSender<String>>,
}

struct App {
    rooms: Mutex<HashMap<String, Room>>,
    next_id: AtomicU32,
    rng: Mutex<Rng>,
}

type Shared = Arc<App>;

fn seeded_rng() -> Rng {
    let t = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    Rng::new(t.as_nanos() as u64 ^ (t.subsec_nanos() as u64) << 17)
}

fn gen_token(rng: &mut Rng) -> String {
    format!("{:08x}{:08x}{:08x}", rng.next_u32(), rng.next_u32(), rng.next_u32())
}

fn gen_code(rng: &mut Rng, taken: &HashMap<String, Room>) -> String {
    loop {
        let code: String = (0..4).map(|_| (b'A' + rng.below(26) as u8) as char).collect();
        if !taken.contains_key(&code) {
            return code;
        }
    }
}

fn send(tx: &UnboundedSender<String>, msg: &ServerMsg) {
    let _ = tx.send(serde_json::to_string(msg).unwrap());
}

/// What the caller has to know after delivering: which player this connection was
/// bound to, and which players stopped existing.
#[derive(Default)]
struct Delivered {
    bound: Option<u32>,
    unbound: Vec<u32>,
}

/// Deliver what the state machine asked for, and schedule what it asked to be
/// reminded of.
fn deliver(
    state: &Shared,
    code: &str,
    rooms: &mut HashMap<String, Room>,
    pending: &UnboundedSender<String>,
    outs: Vec<Out>,
) -> Delivered {
    let mut done = Delivered::default();
    let mut empty = false;
    for out in outs {
        let Some(room) = rooms.get_mut(code) else { continue };
        match out {
            Out::Bound(id) => {
                room.sockets.insert(id, pending.clone());
                done.bound = Some(id);
            }
            Out::Unbound(id) => {
                room.sockets.remove(&id);
                done.unbound.push(id);
            }
            Out::To(id, msg) => match room.sockets.get(&id) {
                Some(tx) => send(tx, &msg),
                // a message for a connection that has not been bound yet, which is
                // how a rejected Create or Join answers
                None => send(pending, &msg),
            },
            Out::All(msg) => {
                let raw = serde_json::to_string(&msg).unwrap();
                for tx in room.sockets.values() {
                    let _ = tx.send(raw.clone());
                }
            }
            Out::Schedule { at_ms, timer } => {
                let delay = at_ms.saturating_sub(now_ms());
                let state = state.clone();
                let code = code.to_string();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                    fire(&state, &code, timer);
                });
            }
            Out::Empty => empty = true,
        }
    }
    if empty {
        rooms.remove(code);
    }
    done
}

fn fire(state: &Shared, code: &str, timer: Timer) {
    let mut rooms = state.rooms.lock().unwrap();
    if !rooms.contains_key(code) {
        return;
    }
    let outs = {
        let mut rng = state.rng.lock().unwrap();
        let room = rooms.get_mut(code).unwrap();
        room.lobby.apply(now_ms(), &mut rng, Event::Fired(timer))
    };
    let (tx, _rx) = unbounded_channel::<String>();
    deliver(state, code, &mut rooms, &tx, outs);
}

fn handle_msg(
    state: &Shared,
    tx: &UnboundedSender<String>,
    session: &mut Option<(String, u32)>,
    raw: &str,
) {
    let Ok(msg) = serde_json::from_str::<ClientMsg>(raw) else {
        send(tx, &ServerMsg::Error { message: "bad message".into() });
        return;
    };
    let mut rooms = state.rooms.lock().unwrap();

    // Which lobby does this message concern? Create makes one; Join and Rejoin name
    // one; everything else uses the session's.
    let code = match (&msg, session.as_ref()) {
        (ClientMsg::Create { .. }, None) => {
            let mut rng = state.rng.lock().unwrap();
            let code = gen_code(&mut rng, &rooms);
            drop(rng);
            rooms.insert(
                code.clone(),
                Room { lobby: Lobby::new(code.clone(), settings()), sockets: HashMap::new() },
            );
            code
        }
        (ClientMsg::Join { code, .. } | ClientMsg::Rejoin { code, .. }, None) => {
            let code = code.trim().to_uppercase();
            if !rooms.contains_key(&code) {
                let message = match msg {
                    ClientMsg::Rejoin { .. } => "session expired",
                    _ => "lobby not found",
                };
                send(tx, &ServerMsg::Error { message: message.into() });
                return;
            }
            code
        }
        (_, Some((code, _))) => code.clone(),
        (ClientMsg::Leave | ClientMsg::Click { .. }, None) => return,
        (_, None) => {
            send(tx, &ServerMsg::Error { message: "not in a lobby".into() });
            return;
        }
    };

    let from = session.as_ref().map(|(_, id)| *id);
    let outs = {
        let mut rng = state.rng.lock().unwrap();
        let id = state.next_id.fetch_add(1, Ordering::Relaxed);
        let token = gen_token(&mut rng);
        let room = rooms.get_mut(&code).unwrap();
        room.lobby.apply(now_ms(), &mut rng, Event::Client { from, msg, id, token })
    };
    let done = deliver(state, &code, &mut rooms, tx, outs);
    if let Some(id) = done.bound {
        *session = Some((code, id));
    }
    // Leave, or a removal that took us with it, frees the socket for a fresh lobby.
    if let Some((_, id)) = session.as_ref() {
        if done.unbound.contains(id) {
            *session = None;
        }
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
        let mut rooms = state.rooms.lock().unwrap();
        if rooms.contains_key(&code) {
            let outs = {
                let mut rng = state.rng.lock().unwrap();
                let room = rooms.get_mut(&code).unwrap();
                room.lobby.apply(now_ms(), &mut rng, Event::Dropped(id))
            };
            deliver(&state, &code, &mut rooms, &tx, outs);
        }
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
    let state: Shared = Arc::new(App {
        rooms: Mutex::new(HashMap::new()),
        next_id: AtomicU32::new(1),
        rng: Mutex::new(seeded_rng()),
    });

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
