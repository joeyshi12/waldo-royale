//! The lobby rules, driven directly. No sockets, no runtime, no real clock: these
//! are the cases that used to need a server and a WebSocket client each.

use waldo_core::{
    lobby::{Event, Lobby, Out, Settings, Timer},
    protocol::{ClientMsg, ServerMsg},
    rng::Rng,
};

/// A lobby plus the bookkeeping a transport would normally own: the clock, the id
/// counter and the rng.
struct Harness {
    lobby: Lobby,
    rng: Rng,
    now: u64,
    next_id: u32,
}

impl Harness {
    fn new(settings: Settings) -> Self {
        Harness {
            lobby: Lobby::new("TEST", settings),
            rng: Rng::new(0xC0FFEE),
            now: 1_000_000,
            next_id: 1,
        }
    }

    fn fixed_mutator() -> Self {
        Harness::new(Settings { mutator: Some(String::new()), ..Settings::default() })
    }

    /// A message from an established player.
    fn from(&mut self, id: u32, msg: ClientMsg) -> Vec<Out> {
        let next = self.next_id;
        self.lobby.apply(
            self.now,
            &mut self.rng,
            Event::Client { from: Some(id), msg, id: next, token: format!("t{next}") },
        )
    }

    /// A message from a connection with no player yet. Returns the id it was bound
    /// to, and everything the machine emitted.
    fn fresh(&mut self, msg: ClientMsg) -> (Option<u32>, Vec<Out>) {
        let id = self.next_id;
        self.next_id += 1;
        let outs = self.lobby.apply(
            self.now,
            &mut self.rng,
            Event::Client { from: None, msg, id, token: format!("t{id}") },
        );
        let bound = outs.iter().find_map(|o| match o {
            Out::Bound(id) => Some(*id),
            _ => None,
        });
        (bound, outs)
    }

    fn create(&mut self, name: &str) -> u32 {
        let (bound, _) = self.fresh(ClientMsg::Create { name: name.into() });
        bound.expect("create binds a player")
    }

    fn join(&mut self, name: &str) -> Option<u32> {
        let (bound, _) =
            self.fresh(ClientMsg::Join { code: "TEST".into(), name: name.into() });
        bound
    }

    fn advance(&mut self, ms: u64) {
        self.now += ms;
    }

    fn fire(&mut self, timer: Timer) -> Vec<Out> {
        self.lobby.apply(self.now, &mut self.rng, Event::Fired(timer))
    }

    fn dropped(&mut self, id: u32) -> Vec<Out> {
        self.lobby.apply(self.now, &mut self.rng, Event::Dropped(id))
    }
}

fn messages(outs: &[Out]) -> Vec<&ServerMsg> {
    outs.iter()
        .filter_map(|o| match o {
            Out::To(_, m) | Out::All(m) => Some(m),
            _ => None,
        })
        .collect()
}

fn scheduled(outs: &[Out]) -> Vec<(u64, Timer)> {
    outs.iter()
        .filter_map(|o| match o {
            Out::Schedule { at_ms, timer } => Some((*at_ms, *timer)),
            _ => None,
        })
        .collect()
}

fn round_start(outs: &[Out]) -> (u32, String, u64) {
    for m in messages(outs) {
        if let ServerMsg::RoundStart { seed, mutator, ends_at_ms, .. } = m {
            return (*seed, mutator.clone(), *ends_at_ms);
        }
    }
    panic!("no round_start in {:?}", messages(outs));
}

fn errors(outs: &[Out]) -> Vec<String> {
    messages(outs)
        .into_iter()
        .filter_map(|m| match m {
            ServerMsg::Error { message } => Some(message.clone()),
            _ => None,
        })
        .collect()
}

/// Where Waldo is this round, derived the way a client derives it: from the seed.
fn waldo_at(seed: u32, mutator: &str) -> [f32; 3] {
    waldo_core::generate_world_opts(seed, waldo_core::mutator_opts(mutator)).waldo.pos
}

#[test]
fn a_full_two_player_game() {
    let mut h = Harness::fixed_mutator();
    let alice = h.create("Alice");
    let bob = h.join("Bob").expect("bob joins");

    h.from(alice, ClientMsg::Configure { rounds: 1, round_secs: 60 });
    let outs = h.from(alice, ClientMsg::Start);
    let (seed, mutator, ends_at) = round_start(&outs);
    assert_eq!(ends_at, h.now + 60_000, "a 60s round ends 60s from now");
    assert_eq!(
        scheduled(&outs),
        vec![(h.now + 60_000, Timer::RoundEnd { gen: 1 })],
        "the round's end is the only thing scheduled"
    );

    // Alice finds Waldo, then Bob, which ends the round early
    let w = waldo_at(seed, &mutator);
    let outs = h.from(alice, ClientMsg::Click { pos: w });
    let hit = messages(&outs)
        .into_iter()
        .find_map(|m| match m {
            ServerMsg::ClickResult { target, points, .. } => Some((target.clone(), *points)),
            _ => None,
        })
        .expect("a click result");
    assert_eq!(hit.0, "waldo");
    assert_eq!(hit.1, waldo_core::rank_points(1) as i32, "first finder gets rank 1 points");

    h.advance(2000);
    let outs = h.from(bob, ClientMsg::Click { pos: w });
    let results = messages(&outs)
        .into_iter()
        .find_map(|m| match m {
            ServerMsg::RoundResult { results, .. } => Some(results.clone()),
            _ => None,
        })
        .expect("everyone found Waldo, so the round ends without waiting for the clock");
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].rank, 1, "results are ordered by score, Alice first");
    assert!(results[0].score > results[1].score);
    assert_eq!(results[1].time_ms, 2000, "Bob found him two seconds in");

    // the intermission timer ends the game, because that was the only round
    let (at, timer) = scheduled(&outs)[0];
    assert_eq!(at, h.now + 8000);
    h.advance(8000);
    let outs = h.fire(timer);
    assert!(
        messages(&outs).iter().any(|m| matches!(m, ServerMsg::GameOver { .. })),
        "one round configured, so the intermission ends the game"
    );
}

#[test]
fn the_round_clock_ends_the_round_when_nobody_finds_him() {
    let mut h = Harness::fixed_mutator();
    let alice = h.create("Alice");
    h.from(alice, ClientMsg::Configure { rounds: 2, round_secs: 30 });
    let outs = h.from(alice, ClientMsg::Start);
    let (_, timer) = scheduled(&outs)[0];

    h.advance(30_000);
    let outs = h.fire(timer);
    let results = messages(&outs)
        .into_iter()
        .find_map(|m| match m {
            ServerMsg::RoundResult { results, .. } => Some(results.clone()),
            _ => None,
        })
        .expect("the clock ends the round");
    assert!(!results[0].found, "nobody found him");
    assert_eq!(results[0].score, 0);
}

#[test]
fn a_stale_round_timer_is_ignored() {
    let mut h = Harness::fixed_mutator();
    let alice = h.create("Alice");
    h.from(alice, ClientMsg::Configure { rounds: 1, round_secs: 60 });
    let outs = h.from(alice, ClientMsg::Start);
    let (seed, mutator, _) = round_start(&outs);
    let (_, stale) = scheduled(&outs)[0];

    // Alice finds him, so the round already finished
    h.from(alice, ClientMsg::Click { pos: waldo_at(seed, &mutator) });

    let outs = h.fire(stale);
    assert!(
        messages(&outs).is_empty(),
        "the round already ended, so its timeout must do nothing: {:?}",
        messages(&outs)
    );
}

#[test]
fn a_dropped_player_keeps_their_seat_until_the_grace_period_expires() {
    let mut h = Harness::new(Settings {
        mutator: Some(String::new()),
        grace_ms: 5_000,
        ..Settings::default()
    });
    let alice = h.create("Alice");
    let bob = h.join("Bob").unwrap();
    h.from(alice, ClientMsg::Configure { rounds: 1, round_secs: 60 });
    h.from(alice, ClientMsg::Start);

    let outs = h.dropped(bob);
    let (at, grace) = scheduled(&outs)
        .into_iter()
        .find(|(_, t)| matches!(t, Timer::Grace { .. }))
        .expect("a grace timer");
    assert_eq!(at, h.now + 5_000);
    assert_eq!(h.lobby.player_count(), 2, "still holding Bob's seat");

    // Bob comes back before it fires, which invalidates the timer
    let (bound, outs) =
        h.fresh(ClientMsg::Rejoin { code: "TEST".into(), token: "t2".into() });
    assert_eq!(bound, Some(bob));
    assert!(
        messages(&outs).iter().any(|m| matches!(m, ServerMsg::Restore { .. })),
        "rejoining mid-round restores the round"
    );

    h.advance(5_000);
    h.fire(grace);
    assert_eq!(h.lobby.player_count(), 2, "the stale grace timer must not evict him");
}

#[test]
fn a_player_who_never_comes_back_is_removed() {
    let mut h = Harness::new(Settings {
        mutator: Some(String::new()),
        grace_ms: 5_000,
        ..Settings::default()
    });
    let alice = h.create("Alice");
    let bob = h.join("Bob").unwrap();

    let outs = h.dropped(bob);
    let (_, grace) = scheduled(&outs).into_iter().find(|(_, t)| matches!(t, Timer::Grace { .. })).unwrap();
    h.advance(5_000);
    let outs = h.fire(grace);
    assert!(
        outs.iter().any(|o| matches!(o, Out::Unbound(id) if *id == bob)),
        "the caller is told to forget his connection"
    );
    assert_eq!(h.lobby.player_count(), 1);
    assert!(h.lobby.player_count() > 0 && !h.lobby.is_empty());
    let _ = alice;
}

#[test]
fn leaving_is_immediate_and_frees_the_connection() {
    let mut h = Harness::fixed_mutator();
    let alice = h.create("Alice");
    let bob = h.join("Bob").unwrap();

    let outs = h.from(bob, ClientMsg::Leave);
    assert!(
        outs.iter().any(|o| matches!(o, Out::Unbound(id) if *id == bob)),
        "no grace period for a deliberate exit"
    );
    assert!(
        !scheduled(&outs).iter().any(|(_, t)| matches!(t, Timer::Grace { .. })),
        "and no grace timer either"
    );
    assert_eq!(h.lobby.player_count(), 1);
    let _ = alice;
}

#[test]
fn the_last_player_leaving_empties_the_lobby() {
    let mut h = Harness::fixed_mutator();
    let alice = h.create("Alice");
    let outs = h.from(alice, ClientMsg::Leave);
    assert!(outs.iter().any(|o| matches!(o, Out::Empty)), "the caller should drop the lobby");
    assert!(h.lobby.is_empty());
}

#[test]
fn only_the_host_may_configure_or_start() {
    let mut h = Harness::fixed_mutator();
    let _alice = h.create("Alice");
    let bob = h.join("Bob").unwrap();

    let outs = h.from(bob, ClientMsg::Configure { rounds: 9, round_secs: 10 });
    assert_eq!(errors(&outs), vec!["only the host can change settings"]);
    let outs = h.from(bob, ClientMsg::Start);
    assert_eq!(errors(&outs), vec!["only the host can start"]);
}

#[test]
fn a_full_lobby_and_a_game_in_progress_both_turn_joiners_away() {
    let mut h = Harness::new(Settings {
        mutator: Some(String::new()),
        max_players: 3,
        ..Settings::default()
    });
    let alice = h.create("Alice");
    assert!(h.join("Bob").is_some());
    assert!(h.join("Carol").is_some());

    let (bound, outs) = h.fresh(ClientMsg::Join { code: "TEST".into(), name: "Dave".into() });
    assert_eq!(bound, None);
    assert_eq!(errors(&outs), vec!["lobby is full"]);

    h.from(alice, ClientMsg::Start);
    let (bound, outs) = h.fresh(ClientMsg::Join { code: "TEST".into(), name: "Dave".into() });
    assert_eq!(bound, None);
    assert_eq!(errors(&outs), vec!["game already in progress"]);
}

#[test]
fn clicks_outside_a_round_are_ignored() {
    let mut h = Harness::fixed_mutator();
    let alice = h.create("Alice");
    let outs = h.from(alice, ClientMsg::Click { pos: [0.0, 0.0, 0.0] });
    assert!(messages(&outs).is_empty(), "no round is running");

    h.from(alice, ClientMsg::Configure { rounds: 1, round_secs: 60 });
    let outs = h.from(alice, ClientMsg::Start);
    let (seed, mutator, _) = round_start(&outs);
    h.from(alice, ClientMsg::Click { pos: waldo_at(seed, &mutator) }); // ends the round

    let outs = h.from(alice, ClientMsg::Click { pos: [0.0, 0.0, 0.0] });
    assert!(messages(&outs).is_empty(), "the round is over, so a click does nothing");
}

#[test]
fn a_miss_costs_and_waldo_is_only_worth_finding_once() {
    let mut h = Harness::fixed_mutator();
    let alice = h.create("Alice");
    h.join("Bob").unwrap(); // so finding Waldo does not end the round
    h.from(alice, ClientMsg::Configure { rounds: 1, round_secs: 60 });
    let outs = h.from(alice, ClientMsg::Start);
    let (seed, mutator, _) = round_start(&outs);
    let w = waldo_at(seed, &mutator);

    let far = [-w[0], -w[1], -w[2]]; // the other side of the planet
    let outs = h.from(alice, ClientMsg::Click { pos: far });
    let (target, points) = messages(&outs)
        .into_iter()
        .find_map(|m| match m {
            ServerMsg::ClickResult { target, points, .. } => Some((target.clone(), *points)),
            _ => None,
        })
        .expect("a verdict");
    assert_eq!(target, "miss");
    assert_eq!(points, -(waldo_core::scoring::MISS_PENALTY as i32));

    h.from(alice, ClientMsg::Click { pos: w });
    let outs = h.from(alice, ClientMsg::Click { pos: w });
    assert!(messages(&outs).is_empty(), "a second click on Waldo is ignored");
}

#[test]
fn a_second_create_on_a_bound_connection_is_refused() {
    let mut h = Harness::fixed_mutator();
    let alice = h.create("Alice");
    let outs = h.from(alice, ClientMsg::Create { name: "Again".into() });
    assert_eq!(errors(&outs), vec!["already in a lobby"]);
}

#[test]
fn every_player_gets_their_own_token_in_their_own_lobby_message() {
    let mut h = Harness::fixed_mutator();
    let alice = h.create("Alice");
    let bob = h.join("Bob").unwrap();
    let outs = h.from(alice, ClientMsg::Configure { rounds: 2, round_secs: 30 });

    let mut seen = Vec::new();
    for o in &outs {
        if let Out::To(id, ServerMsg::Lobby { you, token, players, .. }) = o {
            assert_eq!(id, you, "each lobby message names its own recipient");
            assert!(players.iter().any(|p| p.id == *you));
            seen.push((*id, token.clone()));
        }
    }
    assert_eq!(seen.len(), 2, "one per player, not one broadcast");
    assert_ne!(seen[0].1, seen[1].1, "tokens are private, so they must differ");
    assert!(seen.iter().any(|(id, _)| *id == alice));
    assert!(seen.iter().any(|(id, _)| *id == bob));
}

#[test]
fn the_host_is_the_first_player_and_survives_others_leaving() {
    let mut h = Harness::fixed_mutator();
    let alice = h.create("Alice");
    let bob = h.join("Bob").unwrap();
    let outs = h.from(bob, ClientMsg::Leave);
    let hosts: Vec<u32> = messages(&outs)
        .into_iter()
        .filter_map(|m| match m {
            ServerMsg::Lobby { players, .. } => {
                players.iter().find(|p| p.is_host).map(|p| p.id)
            }
            _ => None,
        })
        .collect();
    assert!(!hosts.is_empty());
    assert!(hosts.iter().all(|id| *id == alice), "Alice stays host");
}
