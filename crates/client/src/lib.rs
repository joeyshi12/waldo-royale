//! The browser client: Leptos renders every screen, three-d renders the
//! planet, waldo-core generates the world.

mod net;
mod render;
mod state;
mod ui;

use leptos::prelude::*;
use wasm_bindgen::prelude::*;

#[wasm_bindgen(start)]
pub fn main() {
    console_error_panic_hook::set_once();
    let ui_state = state::Ui::new();
    let game = state::new_shared();

    render::spawn(ui_state, game.clone());

    // a page refresh mid-game rejoins automatically
    if let Some((code, token)) = state::load_session() {
        net::open_socket(
            ui_state,
            game.clone(),
            waldo_core::protocol::ClientMsg::Rejoin { code, token },
        );
    }

    let wrapped = send_wrapper::SendWrapper::new(game);
    mount_to_body(move || ui::App(ui::AppProps { ui: ui_state, game: wrapped.clone() }));
}
