# Contributing

Thanks for your interest in eztrading! This is an early-stage project, so
there is plenty to do and the bar for a first contribution is low.

## Getting set up

```sh
git clone <your fork>
cd eztrading
cargo build --workspace
cargo test --workspace
cargo run -p agent -- check examples/threshold.sm
cargo run -p studio          # Strategy Studio on http://127.0.0.1:8787
```

You need a recent stable Rust toolchain. The servers additionally need MySQL
and a C++ toolchain to run end to end (see the README); you do not need either
to work on the compiler or the agent.

## Where to help

* **Database schema**: the MySQL schema is referenced from `servers/com/src/db`
  but is not in the repository. Reconstructing it (and a `docker-compose` for
  local development) would unblock running the whole platform.
* **Compiler semantics** (`core/transpiler`): member access, call arguments and
  `goto` targets are not verified. Real errors for these make both users and
  `ezt-agent` better.
* **SML standard library** (`package/libs/std.sm`, `package/common`): indicators,
  order types, cash accounting in `SandBox`.
* **`ezt-agent`**: an evaluation set of requests, a backtesting tool for the
  agent to call (Studio's simulator is a good base), better prompts.
* **Deployments**: an execution backend behind the deployment lifecycle that talks
  to `srvman`/`jobsrv` (needs the missing MySQL schema) or to a real broker;
  alerts and notifications; a real-service test of the Twelve Data feed.
* **Strategy Studio**: more of SML in the simulator (`history()`, `record`),
  multi-symbol backtests, an offline bundle of the CDN libraries.
* Tests for the servers; Windows support; the Studio web UI (more tests, accessibility).

## Pull requests

* Keep a change focused and explain *why* in the description.
* `cargo build --workspace` and `cargo test --workspace` must pass; add a test
  for new behavior where practical. New SML examples must pass
  `ezt-agent check`, and a `.test.json` that passes in Studio is welcome.
* If you touch `core/studio/assets/*.js`, run `node --check` on the file; CI does.
* If you change the SML grammar or the standard library, update
  [docs/sml-language.md](docs/sml-language.md); the agent embeds it in its
  prompt, so stale docs make it write wrong code.
* Do not commit credentials, certificates or machine-specific paths. Use
  `*.example` files and environment variables.

By contributing you agree that your contribution is licensed under the
project's [MIT license](LICENSE).
