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
[rendezvous server](https://github.com/joeyshi12/icebreaker) introduces the peers and
carries nothing else. Every peer holds one WebSocket to it for the whole match: the
host's is what keeps the room open and brings a late joiner in mid-match, and a
joiner's is what holds its seat, so a player who dropped has one to come back to.

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
RENDEZVOUS_URL=https://rv.example wasm-pack build crates/client \
  --target web --release --out-dir ../../web/pkg
python3 -m http.server -d web 8017     # anything that serves static files
cargo test --workspace --exclude waldo-client
```

`RENDEZVOUS_URL` is compiled in and is where the client looks for the rendezvous
server. Give it the http origin, as above; the client swaps in `ws://` or `wss://`
itself, so the same value is what a person would paste into a browser. Left unset it
falls back to the origin the page came from, which only works if one happens to be
there.

## Deploying

`web/` is the whole site: a page, a wasm bundle and 3 MB of scenery, 63 files and
about 5 MB. There is nothing to run, so a static host is enough. CI builds it on every
push and attaches it as the `site` artifact, so a deploy is a download, an unpack and an
upload.

Two headers are not optional, and both were carried by a `web/_headers` file while the
site was on Cloudflare Pages. Whatever serves the bundle now has to send them itself.

**`Cache-Control: no-cache`, on everything.** A browser holding a cached bundle talks to
a peer running newer code, and the host is another player rather than a server, so there
is nothing in the middle to absorb the mismatch.

**`Content-Type: application/wasm` for `pkg/waldo_client_bg.wasm`.** Browsers refuse to
stream-compile wasm served as anything else. Only the wasm: the js glue beside it has to
stay a module. Most servers set this from the file extension, but not all do.

The bundle is subpath-safe, so it can be served from a directory rather than a domain
root: every href is relative and the wasm is found with
`new URL('waldo_client_bg.wasm', import.meta.url)`.

## CI

`ci.yml` runs the test suite and checks the client compiles for wasm.

`build.yml` does the real release build and uploads `web/` as the `site` artifact. It
also asserts the rendezvous host appears in the built wasm, because `option_env!`
resolves to `None` without complaint: a missing `RENDEZVOUS_URL` would otherwise
produce a bundle that silently signals against the page origin instead of failing.
