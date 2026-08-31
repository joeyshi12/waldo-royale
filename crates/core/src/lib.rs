//! Deterministic world generation and scoring for Waldo Royale.
//! Compiled to wasm32 for the browser and natively for the server;
//! one seed produces the same world on both.

pub mod math;
pub mod protocol;
pub mod rng;
pub mod scoring;
pub mod shapes;
pub mod worldgen;

pub use scoring::{score_find, WALDO_HIT_RADIUS};
pub use worldgen::{generate_world, World};
