# Architecture

eztrading is a Cargo workspace of small servers that talk to each other over a
binary protocol (`servers/com`), plus a compiler for the strategy language.

```
                         ┌──────────────┐   HTTPS (actix, TLS)   ┌─────────────┐
  Studio / CLI   ─────►│    srvman    │◄──────────────────────►│   clients   │
                         │ service mgr  │                        └─────────────┘
                         └──────┬───────┘
          starts, monitors      │ heartbeats (interval / timeout in system.toml)
        ┌───────────────┬───────┴────────┬───────────────┐
        ▼               ▼                ▼               │
   ┌─────────┐    ┌──────────┐     ┌──────────┐          │
   │ datasrv │───►│  jobsrv  │     │ codesrv  │          │
   │ prices  │    │ runs     │     │ builds   │          │
   └────┬────┘    │ strategies│    │ plugins  │          │
        │         └────┬─────┘     └────┬─────┘          │
        ▼              ▼                ▼                ▼
   Twelve Data      plugins (.so/.dylib)      MySQL `eztrading` database
```

## Components

* **`core/transpiler` (`compile`)** — parses SML with a PEG grammar
  (`eztrading.pest`, [pest](https://pest.rs)), builds an AST and emits C++
  (`.h`/`.cpp`) plus a serialized object file (`.smo`) used for separate
  compilation of libraries such as `std.sm`. See [sml-language.md](sml-language.md).
* **`package/`** — the C++ runtime that generated code links against
  (`eztrading.h`, scheduler, tickers, account and JSON helpers) and the SML
  standard library. `pack.sh` copies it to a per-user workspace and runs `make`
  to produce a shared library.
* **`servers/com`** — everything shared: the packet/handshake protocol and the
  request/reply enums of each server (`net/`), data types such as time series
  and the memory-mapped `CVec` (`data/`), the data-provider trait, and MySQL
  access (`db/`).
* **`servers/srvman`** — reads `system.toml`, launches `datasrv`, `jobsrv` and
  `codesrv` instances (through `ssh`, using the `user`/`host` entries, even on localhost), watches
  their heartbeats, routes requests (upload, build, load plugin, launch, stop,
  subscribe) and exposes the HTTPS API to clients.
* **`servers/datasrv`** — fetches market data through a provider (currently
  Twelve Data over REST and WebSocket) and streams updates to listeners.
* **`servers/codesrv`** — receives SML source, runs the build script (`pack.sh`)
  and stores the resulting plugin; can publish it with a price.
* **`servers/jobsrv`** — loads plugins, launches *machines* for users with their
  parameters, feeds them price updates and enforces a per-instance capacity.
* **`servers/cli`** — an interactive terminal (rustyline) that connects to any
  of the servers.
* **`core/agent` (`ezt-agent`)** — the AI strategy agent; see [agent.md](agent.md).
* **`core/broker`** — the brokerage layer Studio's runtime trades through; see
  [brokers.md](brokers.md).
* **`core/studio` (`ezt-studio`)** — the browser editor: analysis, diagrams, a preview
  simulator and test runner built on the compiler's grammar, plus a small JSON API;
  see [studio.md](studio.md). It also contains a paper-trading runtime (`live.rs`:
  deployments over price feeds, event-sourced persistence, an audit log, risk guards)
  that is independent of the servers above; see [live.md](live.md).

## Life of a strategy

1. A user (or `ezt-agent`) writes SML.
2. `codesrv` runs `compile` to produce C++, then the toolchain builds a shared
   library plugin and stores it.
3. `srvman` asks a `jobsrv` to `LoadPlugin` and `Launch` a machine for a user
   with their parameters (`param`s in the SML file).
4. `datasrv` pushes price updates; `jobsrv` evaluates each process on its
   `every ...` schedule and moves its state machine along `goto ... on` guards.
5. Orders go through an `Account`. In the C++ runtime that is `SandBox`, an
   in-memory simulation that tracks share counts. Studio's runtime sends orders
   to a `Broker` instead: a simulated account, or a brokerage described by a JSON
   file (`core/broker`, see [brokers.md](brokers.md)).

## Not in this repository (yet)

* The MySQL schema (`users`, `permissions`, `plugins`, ... are referenced from
  `servers/com/src/db`).
* A real execution engine in `core/engine` (it is a stub).
* Verified broker integrations. `core/broker` has an adapter that can drive an
  HTTP brokerage from a JSON description, but it was only tested against local
  imitations. The C++ runtime's `SandBox` keeps positions in memory and does not
  model cash, fills or fees, and has no limit/stop orders.
