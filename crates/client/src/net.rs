//! Message handling: turns a ServerMsg into UI signals and renderer state.
//!
//! How the message arrived is `netplay`'s business. This module is the half that is
//! the same whether we host the lobby or joined one, which is why the host feeds its
//! own lobby's output through here too.

use crate::state::{now_ms, Screen, Shared, Ui};
use leptos::prelude::*;
use std::rc::Rc;
use waldo_core::protocol::{ClientMsg, ServerMsg};
use waldo_core::scoring;

pub fn send(game: &Shared, msg: &ClientMsg) {
    crate::netplay::send(game, msg);
}

pub(crate) fn handle(ui: Ui, game: &Shared, msg: ServerMsg) {
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

/// Deliberately exit the lobby: no reconnect, straight back to the menu.
pub fn leave(ui: Ui, game: &Shared) {
    send(game, &ClientMsg::Leave);
    {
        let mut g = game.borrow_mut();
        g.session = None;
        g.playing = false;
        g.found = false;
        g.world = None;
        g.round_id += 1; // renderer tears the scene down
    }
    crate::state::clear_session();
    ui.found_banner.set(String::new());
    ui.found_count.set(String::new());
    ui.screen.set(Screen::Menu);
}

/// Send a guess click for a planet-local position (called from the renderer).
pub fn send_click(game: &Shared, pos: [f32; 3]) {
    send(game, &ClientMsg::Click { pos });
}

/// True while clicking is meaningful.
pub fn can_click(game: &Shared) -> bool {
    let g = game.borrow();
    g.playing && !g.found && g.link.as_ref().map(|l| l.is_open()).unwrap_or(false)
}

pub const _HIT_RADIUS: f32 = scoring::WALDO_HIT_RADIUS;
