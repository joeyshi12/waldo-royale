//! The lobby state machine: every rule of the game, and no I/O.
//!
//! Nothing in here reads a clock, sleeps, allocates an id, or sends a byte. A
//! caller feeds it [`Event`]s with the current time and gets back [`Out`]s to
//! deliver, which is what lets the same rules run behind a WebSocket server or
//! inside a host's browser over a data channel.
//!
//! Three things a caller must supply because the machine will not invent them:
//! the current time in milliseconds, player ids and session tokens, and an
//! [`Rng`] for round seeds and mutator choice.

use crate::{
    bonus_points,
    math::v3,
    mutator_opts,
    protocol::{ClientMsg, Config, PlayerInfo, RoundEntry, ServerMsg, Standing},
    rank_points,
    rng::Rng,
    score_round, scoring, WALDO_HIT_RADIUS,
};

/// A timer the caller is asked to fire back later. Each carries the generation it
/// was scheduled in, so a timer that has been overtaken is ignored on arrival
/// rather than having to be cancelled.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Timer {
    /// The round's time limit ran out.
    RoundEnd { gen: u64 },
    /// The pause after a round's results is over.
    Intermission { gen: u64 },
    /// A disconnected player's grace period expired.
    Grace { player: u32, gen: u64 },
}

/// Something the caller must do. Anything the machine cannot do itself.
#[derive(Clone, Debug)]
pub enum Out {
    /// Deliver to one player.
    To(u32, ServerMsg),
    /// Deliver to every player in the lobby.
    All(ServerMsg),
    /// The connection that raised this event now belongs to this player.
    Bound(u32),
    /// This player is gone for good; forget the connection bound to them.
    Unbound(u32),
    /// Feed `Event::Fired(timer)` back at or after this absolute millisecond.
    Schedule { at_ms: u64, timer: Timer },
    /// Nobody is left; the caller should drop the lobby.
    Empty,
}

/// Something that happened. `Client` wraps the wire protocol; the rest are things
/// the transport knows and the protocol does not.
#[derive(Clone, Debug)]
pub enum Event {
    /// A protocol message from a player, or from a connection with no player yet.
    /// `id` and `token` are used only by `Create` and `Join`, which need an
    /// identity the machine cannot mint.
    Client { from: Option<u32>, msg: ClientMsg, id: u32, token: String },
    /// A connection dropped. The player keeps their seat for the grace period.
    Dropped(u32),
    /// A timer the caller was asked to schedule has come due.
    Fired(Timer),
}

/// Knobs that were environment variables when this only ran on a server.
#[derive(Clone, Debug)]
pub struct Settings {
    pub intermission_ms: u64,
    pub grace_ms: u64,
    /// Forces every round's mutator instead of picking one at random. Used by the
    /// end to end tests, which cannot see through a random world.
    pub mutator: Option<String>,
    pub max_players: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { intermission_ms: 8000, grace_ms: 60_000, mutator: None, max_players: 12 }
    }
}

#[derive(PartialEq, Clone, Copy, Debug)]
pub enum Phase {
    Waiting,
    Playing,
    Intermission,
}

struct Player {
    id: u32,
    name: String,
    /// Private session token for reconnecting.
    token: String,
    connected: bool,
    /// Bumped on reconnect to invalidate a pending grace timer.
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
    fn new(id: u32, name: String, token: String) -> Self {
        Player {
            id,
            name,
            token,
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
        }
    }

    fn round_score(&self) -> u32 {
        score_round(self.found_rank, self.misses, self.wenda, self.woof, self.wizard, self.odlaw)
    }

    fn reset_round(&mut self) {
        self.found_rank = 0;
        self.found_at_ms = -1;
        self.misses = 0;
        self.wenda = false;
        self.woof = false;
        self.wizard = false;
        self.odlaw = false;
    }
}

pub struct Lobby {
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
    /// Actual round length in ms; mutators may shorten it.
    round_len_ms: u64,
    /// Bumped whenever a scheduled round or intermission timer becomes stale.
    timer_gen: u64,
    settings: Settings,
}

impl Lobby {
    pub fn new(code: impl Into<String>, settings: Settings) -> Self {
        Lobby {
            code: code.into(),
            players: Vec::new(),
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
            settings,
        }
    }

    pub fn code(&self) -> &str {
        &self.code
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn is_empty(&self) -> bool {
        self.players.is_empty()
    }

    pub fn player_count(&self) -> usize {
        self.players.len()
    }

    /// The only way in. `now_ms` is wall-clock milliseconds; `rng` seeds rounds.
    pub fn apply(&mut self, now_ms: u64, rng: &mut Rng, event: Event) -> Vec<Out> {
        let mut out = Vec::new();
        match event {
            Event::Client { from, msg, id, token } => {
                self.client(now_ms, rng, from, msg, id, token, &mut out)
            }
            Event::Dropped(id) => self.dropped(now_ms, rng, id, &mut out),
            Event::Fired(timer) => self.fired(now_ms, rng, timer, &mut out),
        }
        if self.players.is_empty() {
            out.push(Out::Empty);
        }
        out
    }

    fn client(
        &mut self,
        now_ms: u64,
        rng: &mut Rng,
        from: Option<u32>,
        msg: ClientMsg,
        id: u32,
        token: String,
        out: &mut Vec<Out>,
    ) {
        match msg {
            ClientMsg::Create { name } => {
                if from.is_some() {
                    return err(out, id, "already in a lobby");
                }
                // A lobby the caller just created is empty; anything else means the
                // connection is trying to create a second one.
                if !self.players.is_empty() {
                    return err(out, id, "already in a lobby");
                }
                self.players.push(Player::new(id, sanitize_name(&name), token));
                out.push(Out::Bound(id));
                self.broadcast_lobby(out);
            }

            ClientMsg::Join { name, .. } => {
                if from.is_some() {
                    return err(out, id, "already in a lobby");
                }
                if self.phase != Phase::Waiting {
                    return err(out, id, "game already in progress");
                }
                if self.players.len() >= self.settings.max_players {
                    return err(out, id, "lobby is full");
                }
                self.players.push(Player::new(id, sanitize_name(&name), token));
                out.push(Out::Bound(id));
                self.broadcast_lobby(out);
            }

            ClientMsg::Rejoin { token: offered, .. } => {
                if from.is_some() {
                    return err(out, id, "already in a lobby");
                }
                let Some(p) = self.players.iter_mut().find(|p| p.token == offered) else {
                    return err(out, id, "session expired");
                };
                p.connected = true;
                p.disconnect_gen += 1;
                let rejoined = p.id;
                let restore = (self.phase == Phase::Playing).then(|| self.restore_for(rejoined));
                out.push(Out::Bound(rejoined));
                self.broadcast_lobby(out);
                if let Some(r) = restore.flatten() {
                    out.push(Out::To(rejoined, r));
                }
            }

            ClientMsg::Leave => {
                let Some(id) = from else { return };
                self.hard_remove(now_ms, rng, id, out);
            }

            ClientMsg::Configure { rounds, round_secs } => {
                let Some(id) = from else { return err(out, id, "not in a lobby") };
                if !self.is_host(id) {
                    return err(out, id, "only the host can change settings");
                }
                if self.phase != Phase::Waiting {
                    return err(out, id, "cannot change settings mid-game");
                }
                self.config = Config { rounds, round_secs }.clamped();
                self.broadcast_lobby(out);
            }

            ClientMsg::Start => {
                let Some(id) = from else { return err(out, id, "not in a lobby") };
                if !self.is_host(id) {
                    return err(out, id, "only the host can start");
                }
                if self.phase != Phase::Waiting {
                    return err(out, id, "game already running");
                }
                for p in &mut self.players {
                    p.total = 0;
                }
                self.round = 0;
                self.start_round(now_ms, rng, out);
            }

            ClientMsg::Click { pos } => {
                let Some(id) = from else { return };
                self.click(now_ms, rng, id, pos, out);
            }
        }
    }

    fn click(
        &mut self,
        now_ms: u64,
        rng: &mut Rng,
        id: u32,
        pos: [f32; 3],
        out: &mut Vec<Out>,
    ) {
        if self.phase != Phase::Playing {
            return;
        }
        if !pos.iter().all(|c| c.is_finite()) {
            return;
        }
        let round_len_ms = self.round_len_ms;
        let remaining = self.round_ends_ms.saturating_sub(now_ms);
        let click = v3(pos[0], pos[1], pos[2]);
        let waldo = v3(self.waldo[0], self.waldo[1], self.waldo[2]);

        // resolve the click target: Waldo first, then the nearest cast member in
        // range, otherwise a miss
        let cast_hit: Option<String> = self
            .cast
            .iter()
            .map(|(role, p)| (role.clone(), click.distance(v3(p[0], p[1], p[2]))))
            .filter(|(_, d)| *d <= WALDO_HIT_RADIUS)
            .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
            .map(|(role, _)| role);
        let found_before = self.players.iter().filter(|p| p.found_rank > 0).count() as u32;
        let total_players = self.players.len() as u32;

        let Some(p) = self.players.iter_mut().find(|p| p.id == id) else { return };
        let hit_waldo = click.distance(waldo) <= WALDO_HIT_RADIUS;

        let target: &str;
        let points: i32;
        if hit_waldo && p.found_rank == 0 {
            p.found_rank = found_before + 1;
            p.found_at_ms = (round_len_ms as i64).saturating_sub(remaining as i64).max(0);
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
                "wenda" => scoring::BONUS_WENDA as i32,
                "woof" => scoring::BONUS_WOOF as i32,
                "wizard" => scoring::BONUS_WIZARD as i32,
                _ => -(scoring::ODLAW_PENALTY as i32),
            };
        } else {
            p.misses += 1;
            target = "miss";
            points = -(scoring::MISS_PENALTY as i32);
        }

        out.push(Out::To(
            id,
            ServerMsg::ClickResult {
                target: target.to_string(),
                points,
                round_score: p.round_score(),
                misses: p.misses,
            },
        ));

        if target == "waldo" {
            let found = found_before + 1;
            out.push(Out::All(ServerMsg::PlayerFound { id, found, total: total_players }));
            if self.all_connected_found() {
                self.finish_round(now_ms, rng, out);
            }
        }
    }

    /// A connection dropped: keep the player for the grace period so they can rejoin.
    fn dropped(&mut self, now_ms: u64, rng: &mut Rng, id: u32, out: &mut Vec<Out>) {
        let Some(p) = self.players.iter_mut().find(|p| p.id == id) else { return };
        p.connected = false;
        p.disconnect_gen += 1;
        let gen = p.disconnect_gen;
        self.broadcast_lobby(out);
        out.push(Out::Schedule {
            at_ms: now_ms + self.settings.grace_ms,
            timer: Timer::Grace { player: id, gen },
        });
        if self.phase == Phase::Playing && self.all_connected_found() {
            self.finish_round(now_ms, rng, out);
        }
    }

    fn fired(&mut self, now_ms: u64, rng: &mut Rng, timer: Timer, out: &mut Vec<Out>) {
        match timer {
            Timer::RoundEnd { gen } => {
                if self.timer_gen == gen && self.phase == Phase::Playing {
                    self.finish_round(now_ms, rng, out);
                }
            }
            Timer::Intermission { gen } => {
                if self.timer_gen != gen || self.phase != Phase::Intermission {
                    return;
                }
                if self.round >= self.config.rounds {
                    out.push(Out::All(ServerMsg::GameOver { leaderboard: self.leaderboard() }));
                    self.phase = Phase::Waiting;
                    self.round = 0;
                    self.broadcast_lobby(out);
                } else {
                    self.start_round(now_ms, rng, out);
                }
            }
            Timer::Grace { player, gen } => {
                let still_gone = self
                    .players
                    .iter()
                    .find(|p| p.id == player)
                    .map(|p| !p.connected && p.disconnect_gen == gen)
                    .unwrap_or(false);
                if still_gone {
                    self.hard_remove(now_ms, rng, player, out);
                }
            }
        }
    }

    /// Remove a player for real: grace expired, or they left deliberately.
    fn hard_remove(&mut self, now_ms: u64, rng: &mut Rng, id: u32, out: &mut Vec<Out>) {
        let before = self.players.len();
        self.players.retain(|p| p.id != id);
        if self.players.len() == before {
            return; // already gone
        }
        out.push(Out::Unbound(id));
        if self.players.is_empty() {
            return;
        }
        self.broadcast_lobby(out);
        if self.phase == Phase::Playing && self.all_connected_found() {
            self.finish_round(now_ms, rng, out);
        }
    }

    fn start_round(&mut self, now_ms: u64, rng: &mut Rng, out: &mut Vec<Out>) {
        self.round += 1;
        self.phase = Phase::Playing;
        self.seed = rng.next_u32();
        self.mutator = self.settings.mutator.clone().unwrap_or_else(|| {
            match rng.below(9) {
                0 => "night",
                1 => "lightning",
                2 => "crowded",
                3 => "tiny",
                _ => "",
            }
            .to_string()
        });
        let secs = if self.mutator == "lightning" {
            (self.config.round_secs / 3).clamp(15, 45)
        } else {
            self.config.round_secs
        };
        self.round_len_ms = secs as u64 * 1000;
        self.round_ends_ms = now_ms + self.round_len_ms;
        let world = crate::generate_world_opts(self.seed, mutator_opts(&self.mutator));
        self.waldo = world.waldo.pos;
        self.cast = world.cast.iter().map(|c| (c.role.clone(), c.pos)).collect();
        for p in &mut self.players {
            p.reset_round();
        }
        self.timer_gen += 1;
        out.push(Out::All(ServerMsg::RoundStart {
            round: self.round,
            total_rounds: self.config.rounds,
            seed: self.seed,
            round_secs: secs,
            mutator: self.mutator.clone(),
            ends_at_ms: self.round_ends_ms,
        }));
        out.push(Out::Schedule {
            at_ms: self.round_ends_ms,
            timer: Timer::RoundEnd { gen: self.timer_gen },
        });
    }

    fn finish_round(&mut self, now_ms: u64, _rng: &mut Rng, out: &mut Vec<Out>) {
        self.phase = Phase::Intermission;
        self.timer_gen += 1; // invalidate the round timeout

        let waldo = self.waldo;
        let mut results: Vec<RoundEntry> = self
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

        let pause = self.settings.intermission_ms;
        out.push(Out::All(ServerMsg::RoundResult {
            round: self.round,
            waldo,
            results,
            next_in_ms: pause,
        }));
        out.push(Out::Schedule {
            at_ms: now_ms + pause,
            timer: Timer::Intermission { gen: self.timer_gen },
        });
    }

    fn all_connected_found(&self) -> bool {
        let connected: Vec<_> = self.players.iter().filter(|p| p.connected).collect();
        !connected.is_empty() && connected.iter().all(|p| p.found_rank > 0)
    }

    fn is_host(&self, id: u32) -> bool {
        self.players.first().map(|p| p.id) == Some(id)
    }

    fn leaderboard(&self) -> Vec<Standing> {
        let mut rows: Vec<Standing> = self
            .players
            .iter()
            .map(|p| Standing { id: p.id, name: p.name.clone(), total: p.total })
            .collect();
        rows.sort_by(|a, b| b.total.cmp(&a.total));
        rows
    }

    fn restore_for(&self, id: u32) -> Option<ServerMsg> {
        let p = self.players.iter().find(|p| p.id == id)?;
        Some(ServerMsg::Restore {
            round: self.round,
            total_rounds: self.config.rounds,
            seed: self.seed,
            round_secs: (self.round_len_ms / 1000) as u32,
            mutator: self.mutator.clone(),
            ends_at_ms: self.round_ends_ms,
            found_rank: p.found_rank,
            misses: p.misses,
            wenda: p.wenda,
            woof: p.woof,
            wizard: p.wizard,
            odlaw: p.odlaw,
        })
    }

    fn snapshot_for(&self, id: u32) -> Option<ServerMsg> {
        let me = self.players.iter().find(|p| p.id == id)?;
        Some(ServerMsg::Lobby {
            code: self.code.clone(),
            you: id,
            token: me.token.clone(),
            players: self
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
            config: self.config,
        })
    }

    /// Each player gets a Lobby message carrying their own id and token, so this
    /// cannot be one broadcast.
    fn broadcast_lobby(&self, out: &mut Vec<Out>) {
        for p in &self.players {
            if let Some(msg) = self.snapshot_for(p.id) {
                out.push(Out::To(p.id, msg));
            }
        }
    }
}

fn err(out: &mut Vec<Out>, id: u32, message: &str) {
    out.push(Out::To(id, ServerMsg::Error { message: message.into() }));
}

pub fn sanitize_name(name: &str) -> String {
    let n: String = name.chars().filter(|c| !c.is_control()).take(20).collect();
    let n = n.trim().to_string();
    if n.is_empty() {
        "Player".into()
    } else {
        n
    }
}
