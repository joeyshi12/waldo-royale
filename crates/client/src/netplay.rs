//! Netplay: the game with no server.
//!
//! One player hosts. Their browser runs the [`Lobby`] rules from `waldo-core` and is
//! the only peer everyone else talks to, over a data channel each. Both ends keep their
//! connection to the rendezvous server: the host's holds the room open and carries a late
//! joiner in mid-match, and a joiner's holds its seat.
//!
//! The host is a player too, so its own messages never touch the network: they go
//! straight into the local lobby, and the results come back through the same
//! `net::handle` that a joiner uses. That symmetry is why the UI does not know or
//! care which role it is in.

use leptos::prelude::*;
use std::{cell::RefCell, rc::Rc};

use wasm_bindgen::{closure::Closure, JsCast};
use wasm_bindgen_futures::spawn_local;
use waldo_core::{
    lobby::{Event, Lobby, Out, Settings, Timer},
    protocol::{ClientMsg, ServerMsg},
    rng::Rng,
};

use crate::{
    net,
    peer::{self, Peer},
    rendezvous::{IceServer, Rendezvous},
    state::{now_ms, Screen, Shared, Ui},
};

/// Generous: the host has to gather ICE candidates before it can answer at all.
const ANSWER_MS: i32 = 30_000;

pub enum Link {
    /// We host: our own messages go into the local lobby.
    Host(Rc<RefCell<Host>>),
    /// We joined: our messages go to the host over a data channel. The rendezvous
    /// connection is held rather than read, because dropping it gives up our seat.
    Joiner {
        peer: Rc<Peer>,
        #[allow(dead_code)]
        rendezvous: Rc<Rendezvous>,
    },
}

impl Link {
    pub fn is_open(&self) -> bool {
        match self {
            Link::Host(_) => true,
            Link::Joiner { peer, .. } => peer.is_open(),
        }
    }
}

pub struct Host {
    ui: Ui,
    lobby: Lobby,
    rng: Rng,
    next_id: u32,
    /// The host's own player id.
    me: u32,
    ice: Vec<IceServer>,
    /// The room lives as long as this does, so it is held for the whole session.
    rendezvous: Rc<Rendezvous>,
    /// Channels bound to a player.
    peers: Vec<(u32, Rc<Peer>)>,
    /// Channels answered but not yet claimed by a Join or Rejoin.
    unclaimed: Vec<Rc<Peer>>,
    /// The channel whose message is being processed, so a reply to a connection
    /// with no player yet still reaches it.
    current: Option<Rc<Peer>>,
    open: bool,
}

fn token_for(rng: &mut Rng) -> String {
    format!("{:08x}{:08x}{:08x}", rng.next_u32(), rng.next_u32(), rng.next_u32())
}

fn seeded_rng() -> Rng {
    Rng::new(now_ms() as u64 ^ 0x9e3779b97f4a7c15)
}

// ---------------------------------------------------------------- host side

/// Become the host: reserve a code, then run the lobby locally.
pub fn create(ui: Ui, game: Shared, name: String) {
    spawn_local(async move {
        let rendezvous = match Rendezvous::connect().await {
            Ok(rv) => rv,
            Err(e) => return ui.toast(format!("Could not reach the lobby service: {e}"), "error"),
        };
        let hosted = match rendezvous.host().await {
            Ok(h) => h,
            Err(e) => return ui.toast(format!("Could not open a lobby: {e}"), "error"),
        };
        let mut rng = seeded_rng();
        let me = 1;
        let token = token_for(&mut rng);
        // the server's cap wins: a twelfth player is refused at the join before reaching
        // this lobby, so stop accepting at the same point rather than failing later
        let default = Settings::default();
        let seats = (hosted.max_joiners as usize + 1).min(default.max_players);
        if seats < default.max_players {
            ui.toast(format!("This lobby holds {seats} players."), "info");
        }
        let host = Rc::new(RefCell::new(Host {
            ui,
            lobby: Lobby::new(hosted.code.clone(), Settings { max_players: seats, ..default }),
            rng,
            next_id: me + 1,
            me,
            ice: hosted.ice_servers,
            rendezvous: rendezvous.clone(),
            peers: Vec::new(),
            unclaimed: Vec::new(),
            current: None,
            open: true,
        }));
        game.borrow_mut().link = Some(Link::Host(host.clone()));

        // the host joins its own lobby
        let outs = {
            let mut h = host.borrow_mut();
            let Host { lobby, rng, .. } = &mut *h;
            let ev = Event::Client { from: None, msg: ClientMsg::Create { name }, id: me, token };
            lobby.apply(now_ms() as u64, rng, ev)
        };
        dispatch(ui, &game, &host, outs);
        accept_joiners(ui, game, &host, &rendezvous);
    });
}

/// Runs for the whole life of the lobby rather than only while it fills up, because that is
/// how a dropped player gets back in.
///
/// The handlers hold weak references: the host owns the connection and the connection owns
/// these, so a strong reference here would be a cycle that never frees.
fn accept_joiners(ui: Ui, game: Shared, host: &Rc<RefCell<Host>>, rendezvous: &Rc<Rendezvous>) {
    let (weak_host, weak_rv) = (Rc::downgrade(host), Rc::downgrade(rendezvous));
    rendezvous.on_offer(move |seat, offer| {
        let (game, weak_host, weak_rv) = (game.clone(), weak_host.clone(), weak_rv.clone());
        spawn_local(async move {
            let (Some(host), Some(rendezvous)) = (weak_host.upgrade(), weak_rv.upgrade()) else {
                return;
            };
            let ice = host.borrow().ice.clone();
            let (peer, answer) = match peer::answer(&ice, &offer).await {
                Ok(pair) => pair,
                Err(e) => return ui.toast(format!("A player could not connect: {e}"), "error"),
            };
            if let Err(e) = rendezvous.answer(seat, &answer) {
                return ui.toast(format!("A player could not be let in: {e}"), "error");
            }
            wire_up(ui, &game, &host, peer.clone());
            host.borrow_mut().unclaimed.push(peer);
        });
    });

    let weak_host = Rc::downgrade(host);
    rendezvous.on_lost(move |why| {
        // the room expired, or the connection to the service went: either way no more
        // players are arriving, and the ones already connected are unaffected
        if let Some(host) = weak_host.upgrade() {
            host.borrow_mut().open = false;
        }
        ui.toast(format!("The lobby is no longer reachable: {why}"), "error");
    });

    rendezvous.on_refused(move |e| {
        ui.toast(format!("A player could not be let in: {e}"), "error");
    });
}

/// Route everything one joiner sends into the local lobby.
fn wire_up(ui: Ui, game: &Shared, host: &Rc<RefCell<Host>>, p: Rc<Peer>) {
    let (g, h, chan) = (game.clone(), host.clone(), p.clone());
    p.on_message(move |text| {
        let Ok(msg) = serde_json::from_str::<ClientMsg>(&text) else { return };
        let outs = {
            let mut hb = h.borrow_mut();
            let from = hb.peers.iter().find(|(_, q)| Rc::ptr_eq(q, &chan)).map(|(id, _)| *id);
            let id = hb.next_id;
            hb.next_id += 1;
            hb.current = Some(chan.clone());
            let Host { lobby, rng, .. } = &mut *hb;
            let token = token_for(rng);
            lobby.apply(now_ms() as u64, rng, Event::Client { from, msg, id, token })
        };
        dispatch(ui, &g, &h, outs);
        h.borrow_mut().current = None;
    });

    let (g2, h2, chan2) = (game.clone(), host.clone(), p.clone());
    p.on_close(move || {
        let outs = {
            let mut hb = h2.borrow_mut();
            let Some(id) = hb.peers.iter().find(|(_, q)| Rc::ptr_eq(q, &chan2)).map(|(i, _)| *i)
            else {
                hb.unclaimed.retain(|q| !Rc::ptr_eq(q, &chan2));
                return;
            };
            let Host { lobby, rng, .. } = &mut *hb;
            lobby.apply(now_ms() as u64, rng, Event::Dropped(id))
        };
        dispatch(ui, &g2, &h2, outs);
    });
}

/// Deliver what the lobby asked for: to ourselves through `net::handle`, to a joiner
/// over their channel, and schedule anything it wants to be reminded of.
fn dispatch(ui: Ui, game: &Shared, host: &Rc<RefCell<Host>>, outs: Vec<Out>) {
    for out in outs {
        match out {
            Out::Bound(id) => {
                let mut h = host.borrow_mut();
                if let Some(p) = h.current.clone() {
                    h.peers.retain(|(other, q)| *other != id && !Rc::ptr_eq(q, &p));
                    h.unclaimed.retain(|q| !Rc::ptr_eq(q, &p));
                    h.peers.push((id, p));
                }
            }
            Out::Unbound(id) => {
                let mut h = host.borrow_mut();
                if let Some(i) = h.peers.iter().position(|(other, _)| *other == id) {
                    let (_, p) = h.peers.remove(i);
                    drop(h);
                    p.close();
                }
            }
            Out::To(id, msg) => {
                let me = host.borrow().me;
                if id == me {
                    net::handle(ui, game, msg);
                    continue;
                }
                let target = {
                    let h = host.borrow();
                    h.peers
                        .iter()
                        .find(|(other, _)| *other == id)
                        .map(|(_, p)| p.clone())
                        .or_else(|| h.current.clone())
                };
                if let Some(p) = target {
                    p.send(&serde_json::to_string(&msg).unwrap());
                }
            }
            Out::All(msg) => {
                let raw = serde_json::to_string(&msg).unwrap();
                let peers: Vec<Rc<Peer>> =
                    host.borrow().peers.iter().map(|(_, p)| p.clone()).collect();
                for p in peers {
                    p.send(&raw);
                }
                net::handle(ui, game, msg);
            }
            Out::Schedule { at_ms, timer } => {
                let delay = at_ms.saturating_sub(now_ms() as u64).min(i32::MAX as u64) as i32;
                let (g, h) = (game.clone(), host.clone());
                let cb = Closure::once_into_js(move || fire(ui, g, h, timer));
                let _ = web_sys::window()
                    .unwrap()
                    .set_timeout_with_callback_and_timeout_and_arguments_0(
                        cb.unchecked_ref(),
                        delay,
                    );
            }
            Out::Empty => {
                let mut h = host.borrow_mut();
                h.open = false;
                h.rendezvous.close_room();
            }
        }
    }
}

fn fire(ui: Ui, game: Shared, host: Rc<RefCell<Host>>, timer: Timer) {
    let outs = {
        let mut h = host.borrow_mut();
        let Host { lobby, rng, .. } = &mut *h;
        lobby.apply(now_ms() as u64, rng, Event::Fired(timer))
    };
    dispatch(ui, &game, &host, outs);
}

// -------------------------------------------------------------- joiner side

/// Connect to a host by code and send `first` once the channel opens.
pub fn connect(ui: Ui, game: Shared, code: String, first: ClientMsg) {
    spawn_local(async move {
        let rendezvous = match Rendezvous::connect().await {
            Ok(rv) => rv,
            Err(e) => return ui.toast(format!("Could not reach the lobby service: {e}"), "error"),
        };
        let ice = match rendezvous.ice().await {
            Ok(servers) => servers,
            Err(e) => return ui.toast(format!("Could not reach the lobby service: {e}"), "error"),
        };
        let (p, offer) = match peer::offer(&ice).await {
            Ok(pair) => pair,
            Err(e) => return ui.toast(format!("Could not start connecting: {e}"), "error"),
        };
        if let Err(e) = rendezvous.join(&code, &offer).await {
            if e.is_missing() {
                return ui.toast("No lobby with that code.", "error");
            }
            return ui.toast(format!("Could not join: {e}"), "error");
        }

        // the host's answer arrives on this connection, pushed as soon as it is sent
        let answer = match rendezvous.answered(ANSWER_MS).await {
            Ok(a) => a,
            Err(e) if e.is_missing() => return ui.toast("The lobby closed.", "error"),
            Err(e) => return ui.toast(format!("{e}."), "error"),
        };
        if let Err(e) = peer::accept_answer(&p, &answer).await {
            return ui.toast(format!("Could not connect to the host: {e}"), "error");
        }

        let (g, chan) = (game.clone(), p.clone());
        p.on_message(move |text| {
            if let Ok(msg) = serde_json::from_str::<ServerMsg>(&text) {
                net::handle(ui, &g, msg);
            }
            let _ = &chan;
        });
        let g2 = game.clone();
        p.on_close(move || {
            g2.borrow_mut().link = None;
            g2.borrow_mut().playing = false;
            ui.toast("Lost the connection to the host.", "error");
            ui.screen.set(Screen::Menu);
        });

        // held, not dropped: it is what keeps this seat for us to come back to
        game.borrow_mut().link =
            Some(Link::Joiner { peer: p.clone(), rendezvous: rendezvous.clone() });
        // a few frames of slack saves a race on slower machines
        for _ in 0..20 {
            if p.is_open() {
                break;
            }
            sleep(50).await;
        }
        send(&game, &first);
    });
}

async fn sleep(ms: i32) {
    let promise = js_sys::Promise::new(&mut |resolve, _| {
        let _ = web_sys::window()
            .unwrap()
            .set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms);
    });
    let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
}

// ------------------------------------------------------------------ shared

/// Send one message, whichever end we are.
pub fn send(game: &Shared, msg: &ClientMsg) {
    let link = match &game.borrow().link {
        Some(Link::Joiner { peer, .. }) => {
            peer.send(&serde_json::to_string(msg).unwrap());
            return;
        }
        Some(Link::Host(h)) => h.clone(),
        None => return,
    };
    // our own message: straight into the lobby, no network
    let (ui, outs) = {
        let mut hb = link.borrow_mut();
        let Host { ui, lobby, rng, me, current, .. } = &mut *hb;
        let ui = *ui;
        let token = token_for(rng);
        *current = None;
        let ev = Event::Client { from: Some(*me), msg: msg.clone(), id: *me, token };
        (ui, lobby.apply(now_ms() as u64, rng, ev))
    };
    dispatch(ui, game, &link, outs);
}
