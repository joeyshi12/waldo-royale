//! Deterministic world generation, scoring and the wire protocol.
//! Compiled to wasm32 for the client and natively for the server;
//! one seed produces the same world on both.

pub mod math;
pub mod protocol;
pub mod rng;
pub mod scoring;
pub mod shapes;
pub mod worldgen;

pub use scoring::{bonus_points, rank_points, score_round, WALDO_HIT_RADIUS};
pub use worldgen::{generate_world, generate_world_opts, mutator_opts, GenOptions, World};
