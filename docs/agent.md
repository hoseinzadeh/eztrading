# ezt-agent — the AI strategy agent

`ezt-agent` turns plain-language ideas into SML strategies and keeps the model
honest by checking everything with the real compiler. It uses the Anthropic
Messages API directly (there is no SDK dependency) and is a single binary.

```sh
cargo build -p agent -p compile        # builds target/debug/ezt-agent and target/debug/compile
export ANTHROPIC_API_KEY=sk-ant-...

ezt-agent generate "Buy when the price drops below 10, sell above 15" -o strategy.sm
ezt-agent fix broken.sm                # writes broken.fixed.sm   (or --in-place / -o FILE)
ezt-agent explain strategy.sm          # plain-language explanation and risks; read-only
ezt-agent check strategy.sm            # compiler + lints, no API key, exit code 1 on failure
```

Run it from the repository root, or point it at the library with `--stdlib`
(default `package/libs`) and at the compiler with `--compiler` (default: the
`compile` binary next to `ezt-agent`, then `PATH`).

## How it works

```
 your request ──► model ──► check_strategy(source) ──► compile + lint
                    ▲                                        │
                    └────────── errors with line/col ◄───────┘
                    │
                    └──► submit_strategy(source, summary) ──► accepted only if it passes
```

1. The system prompt contains the [SML language reference](sml-language.md), the
   real standard library (`package/libs/std.sm`) and an example, so the model
   only has to use names that exist.
2. The model has two tools. `check_strategy` compiles the source with the
   transpiler in a throw-away directory and runs lints; it returns `OK` or the
   errors. `submit_strategy` re-runs the same checks and **only accepts a
   strategy that passes**, so the file `ezt-agent` writes has always compiled.
3. On errors the model reads them, edits the strategy and checks again, up to
   `--max-turns` (default 12). If it gives up, or decides the request cannot be
   expressed in SML, no file is written and the exit code is 2.

`explain` sends no tools and never writes files.

## What is checked — and what is not

* **Compiler**: syntax, and the names the transpiler resolves.
* **Lints** (in `core/agent/src/compiler.rs`): a machine exists; every `goto`
  and `launch` names a declared `state`.
* **Policy** (enforced by the harness, not left to the model): no inline C++
  (`{% ... %}`) unless `--allow-cpp`; source is at most 64 KiB.

The transpiler does **not** verify member access, call arguments or types, so a
strategy can pass every check and still misbehave (for example call a method
the library does not have). The agent is told to say so in its summary, but
**read the result and paper-trade it before risking money.** Nothing it
produces is financial advice.

## Options and environment

| Option | Environment | Default | Meaning |
| --- | --- | --- | --- |
| `--model` | `EZT_AGENT_MODEL` | `claude-opus-5-5` | Claude model. `claude-sonnet-5-5` is cheaper and usually enough for simple strategies. |
| `--effort` | `EZT_AGENT_EFFORT` | model default | `low`, `medium`, `high`, `xhigh`, `max`. |
| `--max-turns` | | `12` | Model calls before giving up. Bounds the cost of a run. |
| `--allow-cpp` | | off | Permit inline C++ in the result. Review such output as code. |
| `-v`, `--verbose` | | off | Print each tool call. |
| `--compiler` | `EZT_COMPILER` | sibling `compile`, then `PATH` | SML compiler binary. |
| `--stdlib` | `EZT_STDLIB` | `package/libs` | Directory containing `std.sm`. |
| | `ANTHROPIC_API_KEY` | required except for `check` | API key. |
| | `ANTHROPIC_BASE_URL` | `https://api.anthropic.com` | Point at a proxy or a test double. |

Each run makes up to `--max-turns` paid API calls; the token totals are printed
to stderr when it finishes.

## Safety notes

* Your request and any file given to `fix`/`explain` are passed to the model as
  data inside tagged blocks, and the system prompt tells it to ignore
  instructions found there. That reduces, but does not eliminate, prompt
  injection risk: do not run `fix` on untrusted files with `--allow-cpp`.
* The compiler is only run on the *translation* step (SML → C++ text). Nothing
  the model writes is built or executed by `ezt-agent`.
* The API key is read from the environment and never written to disk or logs.

## Strategy Studio

The same agent powers the AI tab of [Strategy Studio](studio.md), where the result
appears next to the editor, the diagram, the backtest and the tests.

## Development

```sh
cargo test -p agent
```

The agent loop is tested with a scripted model and a fake validator
(`core/agent/src/agent.rs`), the lints and compiler wrapper in
`core/agent/src/compiler.rs`. To try the whole thing without an API key,
point `ANTHROPIC_BASE_URL` at a local server that returns `tool_use` replies.

Ideas that would make it better: an evaluation set of strategy requests with
expected behaviors, a backtesting tool the agent can call to test its own
output, deeper semantic checks in the compiler, and streaming output.
