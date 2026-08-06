mod game;
mod input;
mod net;
mod perlin;
mod player;
pub mod protocol;
mod renderer;
mod world;

use wasm_bindgen::prelude::*;

#[wasm_bindgen(start)]
pub fn start() -> Result<(), JsValue> {
    console_error_panic_hook::set_once();
    game::start()
}
