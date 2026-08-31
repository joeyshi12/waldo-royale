# Waldo Royale

A GeoGuessr-style multiplayer "Where's Waldo" game. Players join a lobby, all
see the **same procedurally generated planet** (donut, knot, cube, asteroid…),
and race to pin Waldo's location. Points are awarded by how close your pin is
to Waldo — closest pin wins the round, highest total wins the game.

## Architecture

The core game logic is written in **Rust** and shared by both sides of the wire,
which guarantees every player sees the identical world and identical scores:

```
waldo-royale/
├── Cargo.toml              # cargo workspace
├── crates/
│   ├── core/               # waldo-core: deterministic worldgen + scoring (pure Rust)
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── rng.rs      # PCG32 — same seed ⇒ same world, everywhere
│   │       ├── math.rs     # Vec3 / Quat helpers
│   │       ├── shapes.rs   # parametric planet meshes (torus, knot, cube, …)
│   │       ├── worldgen.rs # surface sampling, scenery placement, Waldo placement
│   │       ├── scoring.rs  # distance → points (GeoGuessr-style curve)
│   │       └── protocol.rs # lobby/game message types (serde)
│   ├── wasm/               # waldo-wasm: wasm-bindgen bindings for the browser
│   └── server/             # waldo-server: axum HTTP + WebSocket lobby server
└── web/                    # front end (Three.js renders what the core generates)
    ├── index.html
    ├── js/
    │   ├── main.js         # screen flow + game orchestration
    │   ├── net.js          # WebSocket client
    │   ├── render.js       # builds Three.js scene from core world data
    │   └── ui.js           # DOM helpers
    └── pkg/                # wasm-pack output (generated, not committed)
```

- The **client** calls `generate_world(seed)` through WASM and renders the
  result with Three.js. Rendering stays on the GPU; Rust owns *what* the world
  is, not how it is drawn.
- The **server** runs the same `waldo-core` natively: it knows where Waldo is
  for any seed and scores guesses authoritatively. Clients never receive
  Waldo's position during a round, so it cannot be sniffed from the wire.

## Gameplay

1. Host creates a lobby, gets a 4-letter code; friends join with the code.
2. Host picks number of rounds (1–10) and a per-round time limit (10 s–5 min).
3. Each round: everyone gets the same planet. Spin it, zoom in, and **click
   Waldo** as fast as you can. The server judges every click.
4. Scoring: `5000 × (time remaining / round time)`, minus **150 per wrong
   click** (never below 0). Don't find him ⇒ 0. The round ends early once
   everyone has found him.
5. Results after each round, and a win screen crowning the champion at the end.

## Build & run

Prereqs: Rust (stable), `wasm32-unknown-unknown` target, `wasm-pack`, and any
static-file-free browser — the server serves the client.

```sh
# 1. Build the WASM module into web/pkg
wasm-pack build crates/wasm --target web --release --out-dir ../../web/pkg

# 2. Run the server (serves web/ and the WebSocket endpoint)
cargo run -p waldo-server --release
# → http://localhost:8017

# Tests
cargo test -p waldo-core
```

To play with friends over the internet, deploy the server binary + `web/`
directory to any host and open the same URL; lobbies are in-memory (no
database needed). A `Dockerfile` and `compose.yaml` are included:

```sh
docker compose up -d   # builds the image, serves on :8017
```

## Releases

CI (`.gitea/workflows/ci.yml`) runs the test suite on every push and PR.

Publishing (`.gitea/workflows/publish.yml`) is driven by the workspace
version in `Cargo.toml`: bump it and push to `main`, and the workflow builds
and pushes `registry.internal/waldo-royale:<version>` (and `:latest`) to
the Gitea container registry. Pushes that don't change the version publish
nothing — the workflow skips when the version tag already exists.
Registry credentials come from the repo actions secrets `REGISTRY_USER` /
`REGISTRY_TOKEN`.

