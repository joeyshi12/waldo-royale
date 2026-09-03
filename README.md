# Waldo Royale

A multiplayer "Where's Waldo" game. Players join a lobby, get the same
procedurally generated 3D planet, and race to click Waldo. Fastest finder
wins the round; highest total wins the game.

World generation and scoring live in a Rust crate compiled both to WASM for
the browser and natively for the server, so every player sees the identical
world from a shared seed and the server scores clicks authoritatively.

Scenery models are from [Kenney](https://kenney.nl) (CC0).

## Gameplay

1. Host creates a lobby and shares the 4-letter code.
2. Host picks the number of rounds (1-10) and time per round (10 s to 5 min).
3. Drag to spin the planet, scroll to zoom, click to guess. Finding Waldo
   scores by finish order (1000/700/550/...), wrong clicks cost 25.
4. Wenda, Woof's tail and Wizard Whitebeard hide on every planet as bonus
   finds; Odlaw costs 150 if you mistake him for Waldo.
5. Some rounds carry a mutator: night (cursor searchlight), lightning
   (a third of the time), crowded, or tiny planet.
6. Rounds end early once everyone finds Waldo. Win screen at the end.

## Build and run

Requires stable Rust, the `wasm32-unknown-unknown` target, and `wasm-pack`.

```sh
wasm-pack build crates/wasm --target web --release --out-dir ../../web/pkg
cargo run -p waldo-server --release   # serves web/ on :8017
cargo test --workspace
```

Or with Docker:

```sh
docker compose up -d   # serves on :8017
```

## Releases

CI runs the test suite on every push and pull request. Bumping the workspace
version in `Cargo.toml` and pushing to `main` publishes
`registry.internal/waldo-royale:<version>` to the Gitea container
registry; pushes that keep the same version publish nothing. Registry
credentials come from the repo secrets `REGISTRY_USER` and `REGISTRY_TOKEN`.
