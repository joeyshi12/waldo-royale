# Waldo Royale

A multiplayer "Where's Waldo" game. Players join a lobby, get the same
procedurally generated 3D planet, and race to click Waldo. Fastest finder
wins the round; highest total wins the game.

There is no game server. The whole game is Rust in two crates: `core` (worldgen,
scoring, protocol, and the lobby rules as a state machine with no I/O) and `client`
(a wasm module: Leptos UI, three-d renderer). Every player derives the identical
world from a shared seed.

One player hosts. Their browser runs the lobby rules and is the only peer the others
talk to, over a WebRTC data channel each, and it scores every click. A
[signalling server](https://github.com/joeyshi12/icebreaker) introduces the peers and
carries nothing else; the host keeps polling it for the whole match so a dropped
player can signal their way back in.

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
SIGNAL_URL=https://signal.example wasm-pack build crates/client \
  --target web --release --out-dir ../../web/pkg
python3 -m http.server -d web 8017     # anything that serves static files
cargo test --workspace --exclude waldo-client
```

`SIGNAL_URL` is compiled in and is where the client looks for signalling. Left unset
it falls back to the origin the page came from, which only works if a signalling
server happens to be there.

Or with Docker, which builds the wasm and serves the result with nginx:

```sh
SIGNAL_URL=https://signal.example docker compose up -d --build   # :8017
```

## Releases

CI runs the test suite on every push and pull request. Bumping the workspace
version in `Cargo.toml` and pushing to `main` publishes
`ghcr.io/joeyshi12/waldo-royale:<version>` and `:latest` to the GitHub
Container Registry; pushes that keep the same version publish nothing. The
publish job authenticates with the built-in `GITHUB_TOKEN`, so there are no
registry secrets to configure.
