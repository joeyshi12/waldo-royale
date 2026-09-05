//! All screens, as Leptos components emitting the same CSS classes the
//! stylesheet in index.html defines.

use crate::net::{connect_and, send};
use crate::state::{now_ms, Screen, Shared, Ui};
use send_wrapper::SendWrapper;
use leptos::prelude::*;
use waldo_core::protocol::ClientMsg;

const PLAYER_COLORS: [&str; 12] = [
    "#ff5a5a", "#4fc3f7", "#ffe14a", "#81c784", "#ba68c8", "#ff9e40",
    "#f06292", "#4db6ac", "#a1887f", "#90a4ae", "#dce775", "#7986cb",
];

fn color_for(ui: &Ui, id: u32) -> &'static str {
    let idx = ui
        .players
        .get_untracked()
        .iter()
        .position(|p| p.id == id)
        .unwrap_or(0);
    PLAYER_COLORS[idx % PLAYER_COLORS.len()]
}

fn fmt_time(ms: i64) -> String {
    let s = (ms.max(0) + 999) / 1000;
    format!("{}:{:02}", s / 60, s % 60)
}

#[component]
pub fn App(ui: Ui, game: SendWrapper<Shared>) -> impl IntoView {
    let (name, set_name) = signal(String::new());
    let (code, set_code) = signal(String::new());

    // toast auto-hide: derive visibility from a deadline signal
    let (toast_until, set_toast_until) = signal(0.0f64);
    Effect::new(move |_| {
        ui.status_seq.get();
        if !ui.status.get_untracked().is_empty() {
            set_toast_until.set(now_ms() + 2600.0);
        }
    });

    // 250ms ticker for countdowns (timer bar is driven by the render loop)
    let (tick, set_tick) = signal(0u32);
    let cb = wasm_bindgen::closure::Closure::<dyn FnMut()>::new(move || {
        set_tick.update(|t| *t = t.wrapping_add(1));
    });
    let _ = web_sys::window().unwrap().set_interval_with_callback_and_timeout_and_arguments_0(
        cb.as_ref().unchecked_ref(),
        250,
    );
    cb.forget();

    // StoredValue is Copy, so every handler below is Copy and the view's
    // children closures stay Fn.
    let game_sv: StoredValue<crate::state::Shared, LocalStorage> =
        StoredValue::new_local((*game).clone());

    let create = move |_| {
        game_sv.with_value(|g| connect_and(ui, g.clone(), ClientMsg::Create { name: name.get() }))
    };
    let join = move |_| {
        let c = code.get().trim().to_uppercase();
        if c.len() != 4 {
            ui.toast("lobby codes are 4 letters", "error");
            return;
        }
        game_sv.with_value(|g| connect_and(ui, g.clone(), ClientMsg::Join { code: c, name: name.get() }));
    };
    let start = move |_| game_sv.with_value(|g| send(g, &ClientMsg::Start));
    let configure = move |rounds: u32, secs: u32| {
        game_sv.with_value(|g| send(g, &ClientMsg::Configure { rounds, round_secs: secs }));
    };

    let is_host = move || {
        ui.players
            .get()
            .iter()
            .find(|p| p.id == ui.you.get())
            .map(|p| p.is_host)
            .unwrap_or(false)
    };

    view! {
        // ---------- menu ----------
        <Show when=move || ui.screen.get() == Screen::Menu>
            <div class="screen show">
                <div class="card">
                    <h1>"Waldo "<span class="red">"Royale"</span></h1>
                    <div class="sub">"Same planet. Same Waldo. Fastest finder wins."</div>
                    <div class="field">
                        <label>"Your name"</label>
                        <input
                            maxlength="20"
                            prop:value=name
                            on:input=move |ev| set_name.set(event_target_value(&ev))
                        />
                    </div>
                    <button class="btn" on:click=create>"Create lobby"</button>
                    <div class="divider">"or join a friend"</div>
                    <div class="split">
                        <input
                            maxlength="4"
                            placeholder="Code"
                            style="text-transform:uppercase"
                            prop:value=code
                            on:input=move |ev| set_code.set(event_target_value(&ev))
                        />
                        <button class="btn secondary" on:click=join>"Join"</button>
                    </div>
                </div>
            </div>
        </Show>

        // ---------- lobby ----------
        <Show when=move || ui.screen.get() == Screen::Lobby>
            <div class="screen show">
                <div class="card">
                    <div class="sub" style="margin-bottom:0">"Lobby code"</div>
                    <div id="lobbyCode">{move || ui.lobby_code.get()}</div>
                    <div class="code-hint">"share this code with your friends"</div>
                    <ul id="playerList">
                        <For each=move || ui.players.get() key=|p| p.id let:p>
                            {
                                let pid = p.id;
                                view! {
                                    <li>
                                        <span>
                                            <span
                                                class="swatch"
                                                style:background=color_for(&ui, pid)
                                            ></span>
                                            {p.name.clone()}
                                            {move || if pid == ui.you.get() { " (you)" } else { "" }}
                                            {if !p.connected { " · reconnecting…" } else { "" }}
                                        </span>
                                        <span class="host-tag">
                                            {if p.is_host { "Host" } else { "" }}
                                        </span>
                                    </li>
                                }
                            }
                        </For>
                    </ul>
                    <Show when=is_host>
                        <div class="split">
                            <div class="field">
                                <label>"Rounds"</label>
                                <select on:change=move |ev| {
                                    let r = event_target_value(&ev).parse().unwrap_or(3);
                                    configure(r, ui.config.get_untracked().round_secs);
                                }>
                                    <option value="1">"1"</option>
                                    <option value="2">"2"</option>
                                    <option value="3" selected>"3"</option>
                                    <option value="5">"5"</option>
                                    <option value="7">"7"</option>
                                    <option value="10">"10"</option>
                                </select>
                            </div>
                            <div class="field">
                                <label>"Time per round"</label>
                                <select on:change=move |ev| {
                                    let s = event_target_value(&ev).parse().unwrap_or(90);
                                    configure(ui.config.get_untracked().rounds, s);
                                }>
                                    <option value="30">"30 s"</option>
                                    <option value="60">"1 min"</option>
                                    <option value="90" selected>"1 min 30"</option>
                                    <option value="120">"2 min"</option>
                                    <option value="180">"3 min"</option>
                                    <option value="300">"5 min"</option>
                                </select>
                            </div>
                        </div>
                        <button class="btn" on:click=start>"Start game"</button>
                    </Show>
                    <Show when=move || !is_host()>
                        <div id="waitingMsg">"waiting for the host to start…"</div>
                    </Show>
                </div>
            </div>
        </Show>

        // ---------- in-game HUD ----------
        <Show when=move || ui.screen.get() == Screen::Game>
            <div class="hud" id="roundInfo">
                "Round " {move || ui.round.get()} "/" {move || ui.total_rounds.get()}
                <small>{move || format!("the {} Planet", ui.shape_name.get())}</small>
            </div>
            <div class="hud" id="timerWrap">
                <div id="timerText">{move || fmt_time(ui.time_left_ms.get())}</div>
                <div id="timerBar">
                    <div
                        id="timerFill"
                        style:width=move || {
                            let frac = ui.time_left_ms.get() as f64
                                / ui.round_len_ms.get().max(1) as f64;
                            format!("{}%", (frac.clamp(0.0, 1.0) * 100.0))
                        }
                        style:background=move || {
                            let frac = ui.time_left_ms.get() as f64
                                / ui.round_len_ms.get().max(1) as f64;
                            if frac < 0.15 { "#ff5a5a" }
                            else if frac < 0.4 { "#ffe14a" }
                            else { "#4caf50" }
                        }
                    ></div>
                </div>
            </div>
            <Show when=move || !ui.found_count.get().is_empty()>
                <div class="hud" id="foundCount">{move || ui.found_count.get()}</div>
            </Show>
            <Show when=move || !ui.found_banner.get().is_empty()>
                <div class="hud" id="foundBanner">{move || ui.found_banner.get()}</div>
            </Show>
        </Show>

        // ---------- round results ----------
        <Show when=move || ui.screen.get() == Screen::Results>
            <div class="screen show">
                <div class="card wide">
                    <h1 style="font-size:22px">
                        "Round " {move || ui.results_round.get()} " results"
                    </h1>
                    <table>
                        <thead>
                            <tr>
                                <th>"Player"</th>
                                <th>"Found"</th>
                                <th style="text-align:right">"Bonus"</th>
                                <th style="text-align:right">"Score"</th>
                                <th style="text-align:right">"Total"</th>
                            </tr>
                        </thead>
                        <tbody>
                            <For each=move || ui.results.get() key=|r| r.id let:r>
                                <tr>
                                    <td>
                                        <span
                                            class="swatch"
                                            style:background=color_for(&ui, r.id)
                                        ></span>
                                        {r.name.clone()}
                                    </td>
                                    <td>
                                        {if r.found {
                                            format!("#{} · {:.1}s", r.rank, r.time_ms as f64 / 1000.0)
                                        } else {
                                            "not found".into()
                                        }}
                                        {if r.misses > 0 { format!(" · {}✗", r.misses) } else { String::new() }}
                                    </td>
                                    <td style="text-align:right">
                                        {if r.bonus > 0 { format!("+{}", r.bonus) }
                                         else if r.bonus < 0 { format!("{}", r.bonus) }
                                         else { String::new() }}
                                    </td>
                                    <td class="score">{r.score}</td>
                                    <td style="text-align:right">{r.total}</td>
                                </tr>
                            </For>
                        </tbody>
                    </table>
                    <div id="nextIn">
                        {move || {
                            tick.get();
                            let s = ((ui.next_round_at_ms.get() - now_ms()) / 1000.0).max(0.0);
                            let label = if ui.is_last_round.get() { "final standings" } else { "next round" };
                            if s > 0.2 { format!("{label} in {}…", s.ceil() as u32) } else { "get ready…".into() }
                        }}
                    </div>
                </div>
            </div>
        </Show>

        // ---------- win screen ----------
        <Show when=move || ui.screen.get() == Screen::Final>
            <div class="screen show">
                <div class="card wide" style="text-align:center">
                    {move || {
                        let lb = ui.leaderboard.get();
                        let me = ui.you.get();
                        let my_rank = lb.iter().position(|s| s.id == me);
                        let i_won = my_rank == Some(0);
                        let _ = my_rank; // icon is the game's own mark, not an emoji
                        let headline = if i_won {
                            "You win!".to_string()
                        } else {
                            format!("{} wins", lb.first().map(|s| s.name.as_str()).unwrap_or("Nobody"))
                        };
                        let sub = if i_won {
                            "champion of the galaxy".to_string()
                        } else {
                            let place = my_rank.map(|r| r + 1).unwrap_or(0);
                            let suffix = match place { 1 => "st", 2 => "nd", 3 => "rd", _ => "th" };
                            format!("you finished {place}{suffix} of {}", lb.len())
                        };
                        view! {
                            <img src="./favicon.svg" width="72" height="72" alt=""
                                style="margin-bottom:6px"/>
                            <h1 id="winnerName" style:color=if i_won { "#ffe14a" } else { "#b9c0d8" }>
                                {headline}
                            </h1>
                            <div class="sub" style="margin-bottom:14px">{sub}</div>
                            <ol id="podium">
                                {lb.iter().enumerate().map(|(i, s)| {
                                    let (label, color) = match i {
                                        0 => ("1st", "#ffe14a"),
                                        1 => ("2nd", "#c0c8d8"),
                                        2 => ("3rd", "#d4915d"),
                                        _ => ("", "#9aa3c0"),
                                    };
                                    let label = if label.is_empty() {
                                        format!("{}th", i + 1)
                                    } else {
                                        label.to_string()
                                    };
                                    let you = if s.id == me { " (you)" } else { "" };
                                    view! {
                                        <li>
                                            <span>
                                                <span class="rank" style:color=color
                                                    style:border-color=color>{label}</span>
                                                " " {s.name.clone()} {you}
                                            </span>
                                            <b>{s.total}</b>
                                        </li>
                                    }
                                }).collect_view()}
                            </ol>
                        }
                    }}
                    <button class="btn" on:click=move |_| ui.screen.set(Screen::Lobby)>
                        "Back to lobby"
                    </button>
                </div>
            </div>
        </Show>

        // ---------- toast ----------
        <div
            id="toast"
            class:show=move || {
                tick.get();
                !ui.status.get().is_empty() && now_ms() < toast_until.get()
            }
            class:info=move || ui.status_kind.get() == "info"
        >
            {move || ui.status.get()}
        </div>
    }
}

use wasm_bindgen::JsCast;
