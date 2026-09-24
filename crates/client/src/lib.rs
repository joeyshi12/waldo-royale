//! The browser client: Leptos renders every screen, three-d renders the
//! planet, waldo-core generates the world.

mod net;
mod netplay;
mod peer;
mod render;
mod rendezvous;
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

    // a page refresh mid-game signals its way back to the host
    if let Some((code, token)) = state::load_session() {
        netplay::connect(
            ui_state,
            game.clone(),
            code.clone(),
            waldo_core::protocol::ClientMsg::Rejoin { code, token },
        );
    }

    let wrapped = send_wrapper::SendWrapper::new(game);
    mount_to_body(move || ui::App(ui::AppProps { ui: ui_state, game: wrapped.clone() }));
}
