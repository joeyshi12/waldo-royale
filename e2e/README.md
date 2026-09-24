# End-to-end tests

Headless-browser tests for the wasm client (software WebGL via SwiftShader).

```sh
npm install playwright && npx playwright install chromium
node gameplay.js        # solo round: scene build, timer, drag, click verdict
node e2e.js full        # two players: lobby, round, results, win screen
```

A static file server must be serving `web/` on :8017; there is no game server any
more, and these scripts have not been rewritten for two peers yet, so they will not
pass as they stand. Software rendering starves the main
thread, so waits are generous; the mutator used to be pinned with an environment
variable on the server, and now has to be pinned in the host client instead, to
disable random mutators for deterministic timing.
