# End-to-end tests

Headless-browser tests for the wasm client (software WebGL via SwiftShader).

```sh
npm install playwright && npx playwright install chromium
node gameplay.js        # solo round: scene build, timer, drag, click verdict
node e2e.js full        # two players: lobby, round, results, win screen
```

The server must be running on :8017. Software rendering starves the main
thread, so waits are generous; run `WALDO_MUTATOR=""` on the server to
disable random mutators for deterministic timing.
