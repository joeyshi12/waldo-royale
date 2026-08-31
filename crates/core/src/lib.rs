//! waldo-core: deterministic world generation and scoring for Waldo Royale.
//!
//! Compiled to wasm32 for the browser (rendering data) and natively for the
//! server (authoritative Waldo position + scoring). One seed ⇒ one world,
//! bit-identical on both sides.

pub mod math;
pub mod protocol;
pub mod rng;
pub mod scoring;
pub mod shapes;
pub mod worldgen;

pub use scoring::{score_find, WALDO_HIT_RADIUS};
pub use worldgen::{generate_world, World};

/// Dev-only dependency check: serde_json used by protocol tests.
#[cfg(test)]
mod _ensure_serde_json {}
