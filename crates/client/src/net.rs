//! WebSocket client. Deserializes ServerMsg with the shared waldo-core
//! protocol types and routes them into UI signals and renderer state.

use crate::state::{now_ms, Screen, Shared, Ui};
use leptos::prelude::*;
use std::rc::Rc;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use waldo_core::protocol::{ClientMsg, ServerMsg};
use waldo_core::scoring;
use web_sys::{MessageEvent, WebSocket};

pub fn send(game: &Shared, msg: &ClientMsg) {
    if let Some(ws) = game.borrow().ws.as_ref() {
        let _ = ws.send_with_str(&serde_json::to_string(msg).unwrap());
    }
}

/// Connect (once) and send `first` when open.
pub fn connect_and(ui: Ui, game: Shared, first: ClientMsg) {
    if game.borrow().ws.is_some() {
        send(&game, &first);
        return;
    }
    open_socket(ui, game, first);
}

const MAX_RECONNECT_ATTEMPTS: u32 = 6;

pub fn open_socket(ui: Ui, game: Shared, first: ClientMsg) {
    let loc = web_sys::window().unwrap().location();
    let proto = if loc.protocol().unwrap() == "https:" { "wss" } else { "ws" };
    let url = format!("{proto}://{}/ws", loc.host().unwrap());
    let Ok(ws) = WebSocket::new(&url) else {
        ui.toast("cannot reach the game server", "error");
        return;
    };

    {
        let game = game.clone();
        let onmessage = Closure::<dyn FnMut(MessageEvent)>::new(move |ev: MessageEvent| {
            if let Some(text) = ev.data().as_string() {
                if let Ok(msg) = serde_json::from_str::<ServerMsg>(&text) {
                    handle(ui, &game, msg);
                }
            }
        });
        ws.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
        onmessage.forget();
    }
    {
        let ws2 = ws.clone();
        let first = serde_json::to_string(&first).unwrap();
        let onopen = Closure::<dyn FnMut()>::new(move || {
            let _ = ws2.send_with_str(&first);
        });
        ws.set_onopen(Some(onopen.as_ref().unchecked_ref()));
        onopen.forget();
    }
    {
        let game = game.clone();
        let onclose = Closure::<dyn FnMut()>::new(move || {
            let session = {
                let mut g = game.borrow_mut();
                g.ws = None;
                g.playing = false;
                g.session.clone()
            };
            match session {
                Some((code, token)) => schedule_reconnect(ui, game.clone(), code, token),
                None => {
                    ui.toast("Lost the connection.", "error");
                    ui.screen.set(Screen::Menu);
                }
            }
        });
        ws.set_onclose(Some(onclose.as_ref().unchecked_ref()));
        onclose.forget();
    }
    game.borrow_mut().ws = Some(ws);
}

/// Retry with linear backoff while a session exists.
fn schedule_reconnect(ui: Ui, game: Shared, code: String, token: String) {
    let attempt = {
        let mut g = game.borrow_mut();
        g.reconnect_attempt += 1;
        g.reconnect_attempt
    };
    if attempt > MAX_RECONNECT_ATTEMPTS {
        game.borrow_mut().session = None;
        crate::state::clear_session();
        ui.toast("Could not reconnect.", "error");
        ui.screen.set(Screen::Menu);
        return;
    }
    ui.toast(format!("Connection lost, reconnecting ({attempt}/{MAX_RECONNECT_ATTEMPTS})…"), "info");
    let delay_ms = (attempt * 1200) as i32;
    let cb = Closure::<dyn FnMut()>::new(move || {
        open_socket(ui, game.clone(), ClientMsg::Rejoin {
            code: code.clone(),
            token: token.clone(),
        });
    });
    let _ = web_sys::window().unwrap().set_timeout_with_callback_and_timeout_and_arguments_0(
        cb.as_ref().unchecked_ref(),
        delay_ms,
    );
    cb.forget();
}

fn handle(ui: Ui, game: &Shared, msg: ServerMsg) {
    match msg {
        ServerMsg::Error { message } => {
            if message == "session expired" {
                game.borrow_mut().session = None;
                crate::state::clear_session();
                ui.screen.set(Screen::Menu);
            }
            ui.toast(message, "error");
        }

        ServerMsg::Lobby { code, you, token, players, config } => {
            {
                let mut g = game.borrow_mut();
                g.session = Some((code.clone(), token.clone()));
                g.reconnect_attempt = 0;
            }
            crate::state::save_session(&code, &token);
            ui.lobby_code.set(code);
            ui.you.set(you);
            ui.players.set(players);
            ui.config.set(config);
            // don't yank players off results / win screens (rematch broadcast)
            let s = ui.screen.get_untracked();
            if s == Screen::Menu || s == Screen::Lobby {
                ui.screen.set(Screen::Lobby);
            }
        }

        ServerMsg::RoundStart { round, total_rounds, seed, round_secs, mutator, ends_at_ms } => {
            let t0 = now_ms();
            let world =
                waldo_core::generate_world_opts(seed, waldo_core::mutator_opts(&mutator));
            web_sys::console::debug_1(&format!("worldgen: {:.0} ms", now_ms() - t0).into());
            ui.round.set(round);
            ui.total_rounds.set(total_rounds);
            ui.shape_name.set(world.shape_name.clone());
            ui.round_len_ms.set(round_secs as i64 * 1000);
            ui.found_banner.set(String::new());
            ui.found_count.set(String::new());
            ui.screen.set(Screen::Game);
            let labels: &[(&str, &str)] = &[
                ("night", "Night round · your cursor is a searchlight"),
                ("lightning", "Lightning round · a third of the time"),
                ("crowded", "Crowded round · extra everything"),
                ("tiny", "Tiny planet · everyone is very close together"),
            ];
            if let Some((_, label)) = labels.iter().find(|(m, _)| *m == mutator) {
                ui.toast(*label, "info");
            }
            let mut g = game.borrow_mut();
            g.world = Some(Rc::new(world));
            g.mutator = mutator;
            g.ends_at_ms = (ends_at_ms as f64).min(now_ms() + round_secs as f64 * 1000.0);
            g.playing = true;
            g.found = false;
            g.reveal = None;
            g.pings.clear();
            g.round_id += 1;
        }

        ServerMsg::ClickResult { target, points, round_score: _, misses } => {
            let g_world = game.borrow().world.clone();
            let cast_pos = |role: &str| -> Option<[f32; 3]> {
                g_world.as_ref()?.cast.iter().find(|c| c.role == role).map(|c| c.pos)
            };
            match target.as_str() {
                "waldo" => {
                    game.borrow_mut().found = true;
                    ui.found_banner.set(format!(
                        "Found him! +{points} · bonus hunt: Wenda, Woof's tail, the Wizard"
                    ));
                    if let Some(w) = g_world.as_ref() {
                        game.borrow_mut().pings.push((w.waldo.pos, [255, 225, 74]));
                    }
                }
                "wenda" | "woof" | "wizard" => {
                    let name = match target.as_str() {
                        "wenda" => "Wenda",
                        "woof" => "Woof's tail",
                        _ => "Wizard Whitebeard",
                    };
                    ui.toast(format!("{name}! +{points}"), "info");
                    if let Some(p) = cast_pos(&target) {
                        game.borrow_mut().pings.push((p, [255, 225, 74]));
                    }
                }
                "odlaw" => {
                    ui.toast(format!("That's Odlaw! {points} points"), "error");
                    if let Some(p) = cast_pos("odlaw") {
                        game.borrow_mut().pings.push((p, [232, 160, 32]));
                    }
                }
                _ => {
                    let plural = if misses == 1 { "miss" } else { "misses" };
                    ui.toast(format!("Not him! {points} points ({misses} {plural})"), "error");
                }
            }
        }

        ServerMsg::PlayerFound { found, total, .. } => {
            ui.found_count.set(format!("{found}/{total} found him"));
        }

        ServerMsg::RoundResult { round, waldo, results, next_in_ms } => {
            {
                let mut g = game.borrow_mut();
                g.playing = false;
                g.reveal = Some(waldo);
            }
            ui.results.set(results);
            ui.results_round.set(round);
            ui.next_round_at_ms.set(now_ms() + next_in_ms as f64);
            ui.is_last_round
                .set(ui.round.get_untracked() >= ui.total_rounds.get_untracked());
            ui.screen.set(Screen::Results);
        }

        ServerMsg::Restore {
            round, total_rounds, seed, round_secs, mutator, ends_at_ms,
            found_rank, misses: _, wenda: _, woof: _, wizard: _, odlaw: _,
        } => {
            let world = waldo_core::generate_world_opts(seed, waldo_core::mutator_opts(&mutator));
            ui.round.set(round);
            ui.total_rounds.set(total_rounds);
            ui.shape_name.set(world.shape_name.clone());
            ui.round_len_ms.set(round_secs as i64 * 1000);
            ui.found_count.set(String::new());
            if found_rank > 0 {
                ui.found_banner.set(format!(
                    "Found him! +{} · bonus hunt: Wenda, Woof's tail, the Wizard",
                    waldo_core::rank_points(found_rank)
                ));
            } else {
                ui.found_banner.set(String::new());
            }
            ui.screen.set(Screen::Game);
            ui.toast("Reconnected.", "info");
            let mut g = game.borrow_mut();
            g.world = Some(Rc::new(world));
            g.mutator = mutator;
            g.ends_at_ms = ends_at_ms as f64;
            g.playing = true;
            g.found = found_rank > 0;
            g.reveal = None;
            g.pings.clear();
            g.round_id += 1;
        }

        ServerMsg::GameOver { leaderboard } => {
            ui.leaderboard.set(leaderboard);
            ui.screen.set(Screen::Final);
        }
    }
}

/// Send a guess click for a planet-local position (called from the renderer).
pub fn send_click(game: &Shared, pos: [f32; 3]) {
    send(game, &ClientMsg::Click { pos });
}

/// True while clicking is meaningful.
pub fn can_click(game: &Shared) -> bool {
    let g = game.borrow();
    g.playing && !g.found && g.ws.is_some()
}

pub const _HIT_RADIUS: f32 = scoring::WALDO_HIT_RADIUS;
