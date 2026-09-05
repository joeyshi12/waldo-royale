//! State shared between the Leptos UI, the WebSocket handlers and the
//! three-d render loop. Everything runs on the single wasm thread, so the
//! renderer half lives in an Rc<RefCell> and the UI half in leptos signals.

use leptos::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use waldo_core::protocol::{Config, PlayerInfo, RoundEntry, Standing};
use waldo_core::World;
use web_sys::WebSocket;

#[derive(Clone, Copy, PartialEq)]
pub enum Screen {
    Menu,
    Lobby,
    Game,
    Results,
    Final,
}

/// Reactive state the UI renders from. Copy-cheap: all signals.
#[derive(Clone, Copy)]
pub struct Ui {
    pub screen: RwSignal<Screen>,
    pub status: RwSignal<String>,          // toast text
    pub status_kind: RwSignal<&'static str>, // "error" | "info"
    pub status_seq: RwSignal<u32>,         // bump to retrigger the toast timer

    pub lobby_code: RwSignal<String>,
    pub players: RwSignal<Vec<PlayerInfo>>,
    pub you: RwSignal<u32>,
    pub config: RwSignal<Config>,

    pub round: RwSignal<u32>,
    pub total_rounds: RwSignal<u32>,
    pub shape_name: RwSignal<String>,
    pub time_left_ms: RwSignal<i64>,
    pub round_len_ms: RwSignal<i64>,
    pub found_banner: RwSignal<String>,    // empty = hidden
    pub found_count: RwSignal<String>,     // empty = hidden

    pub results: RwSignal<Vec<RoundEntry>>,
    pub results_round: RwSignal<u32>,
    pub next_round_at_ms: RwSignal<f64>,
    pub is_last_round: RwSignal<bool>,
    pub leaderboard: RwSignal<Vec<Standing>>,
}

impl Ui {
    pub fn new() -> Self {
        Ui {
            screen: RwSignal::new(Screen::Menu),
            status: RwSignal::new(String::new()),
            status_kind: RwSignal::new("error"),
            status_seq: RwSignal::new(0),
            lobby_code: RwSignal::new(String::new()),
            players: RwSignal::new(Vec::new()),
            you: RwSignal::new(0),
            config: RwSignal::new(Config::default()),
            round: RwSignal::new(0),
            total_rounds: RwSignal::new(0),
            shape_name: RwSignal::new(String::new()),
            time_left_ms: RwSignal::new(0),
            round_len_ms: RwSignal::new(1),
            found_banner: RwSignal::new(String::new()),
            found_count: RwSignal::new(String::new()),
            results: RwSignal::new(Vec::new()),
            results_round: RwSignal::new(0),
            next_round_at_ms: RwSignal::new(0.0),
            is_last_round: RwSignal::new(false),
            leaderboard: RwSignal::new(Vec::new()),
        }
    }

    pub fn toast(&self, msg: impl Into<String>, kind: &'static str) {
        self.status.set(msg.into());
        self.status_kind.set(kind);
        self.status_seq.update(|s| *s += 1);
    }
}

/// State the render loop consumes; written by the WebSocket handlers.
pub struct Game {
    pub ws: Option<WebSocket>,
    /// (lobby code, session token) once joined; enables reconnecting.
    pub session: Option<(String, String)>,
    pub reconnect_attempt: u32,
    /// Monotonic id: render loop rebuilds the scene when it changes.
    pub round_id: u32,
    pub world: Option<Rc<World>>,
    pub mutator: String,
    pub ends_at_ms: f64,
    pub playing: bool,
    pub found: bool,
    /// Set by a RoundResult to trigger the reveal (waldo world position).
    pub reveal: Option<[f32; 3]>,
    /// Ping markers requested by click results: (planet-local pos, color).
    pub pings: Vec<([f32; 3], [u8; 3])>,
}

pub type Shared = Rc<RefCell<Game>>;

pub fn new_shared() -> Shared {
    Rc::new(RefCell::new(Game {
        ws: None,
        session: None,
        reconnect_attempt: 0,
        round_id: 0,
        world: None,
        mutator: String::new(),
        ends_at_ms: 0.0,
        playing: false,
        found: false,
        reveal: None,
        pings: Vec::new(),
    }))
}

pub fn now_ms() -> f64 {
    js_sys::Date::now()
}

/// Best-effort session persistence so a page refresh can rejoin.
/// (Storage throws in sandboxed contexts; ignore failures.)
pub fn save_session(code: &str, token: &str) {
    if let Ok(Some(s)) = web_sys::window().unwrap().session_storage() {
        let _ = s.set_item("waldo_session", &format!("{code}:{token}"));
    }
}

pub fn load_session() -> Option<(String, String)> {
    let s = web_sys::window().unwrap().session_storage().ok()??;
    let v = s.get_item("waldo_session").ok()??;
    let (code, token) = v.split_once(':')?;
    Some((code.to_string(), token.to_string()))
}

pub fn clear_session() {
    if let Ok(Some(s)) = web_sys::window().unwrap().session_storage() {
        let _ = s.remove_item("waldo_session");
    }
}
