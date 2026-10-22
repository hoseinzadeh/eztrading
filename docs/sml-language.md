# SML — the State Machine Language

SML is the language eztrading strategies are written in. A strategy is a set of
**machines**; a machine runs one or more **processes**; a process is a **state
machine** whose transitions fire when a condition on market data becomes true.
The `compile` binary (`core/transpiler`) turns `.sm` files into C++, which
`codesrv` builds into a plugin that `jobsrv` schedules.

This page documents what the compiler actually accepts (see
`core/transpiler/src/eztrading.pest` for the grammar and `package/libs/std.sm`
for the built-in library). Each construct below was checked against the
compiler (`compile -I package/libs file.sm`); snippets with `...` are abbreviated.

> **What the compiler checks.** It checks *syntax* and resolves names it
> knows about. It does **not** check member access (`pos.shares`), call
> arguments, or that a `goto` target exists. `ezt-agent` adds a small lint for
> the last one. Always read generated strategies before trading real money.

## File layout

```sml
// Line comment
/* Block comment */

use std::*;          // bring the standard library into scope

machine Name { ... } // one or more machines
event name(...) { ... }   // reusable conditions (top level)
```

Statements end with `;`. Blocks use `{ }`. Identifiers are `[A-Za-z_][A-Za-z0-9_]*`
and must not be keywords: `if else for in machine static process param init
signal let state goto on event return ref top fn const launch emit attach
listen every class pub use mut`, plus the primitive type names.

## Machines

A machine groups parameters, an `init` block and processes.

```sml
machine Threshold {
    param equities: Vec<Symbol> as "Equities";
    param account: Account = SandBox as "Brokerage Account";
    param invest: float = 500.0 as "How Much to Invest";
    param interval: duration = 5 min as "Interval" options {
        5 mins as "Every 5 Minutes",
        1 hour as "Hourly"
    };

    init { launch (watch(sym), WAIT) for sym in equities; }

    process watch(sym: Symbol) every 5 min { ... }
}
```

* `param name: type = default as "Display name";` — a user-editable setting.
  `options { value as "Label", ... }` restricts the choices.
* `init { launch (process(args), STATE) [for x in xs]; }` — starts processes in
  their first state. Only `launch` statements are allowed in `init`.

## Processes and states

```sml
process watch(sym: Symbol) every 5 min {
    let pos: Position;        // local variable
    static count = 0;         // survives between runs
    listen Tickers[sym];      // subscribe to a signal (re-evaluates on updates)

    state WAIT: goto BUY on Tickers[sym] < floor;     // guarded transition

    state BUY: goto HOLD {                            // unconditional, with an action
        pos = account.buy(sym, invest / Tickers[sym].close);
    }

    state HOLD:
        goto SELL on Tickers[sym] > ceiling,          // several transitions,
        goto STOP on Tickers[sym] < stop_loss;        // tried in order

    state SELL: goto WAIT { pos = account.sell(sym, pos.shares); }
    state STOP: goto WAIT { pos = account.sell(sym, pos.shares); }
}
```

* `every <interval>` sets how often the process is evaluated: `every 5 min`,
  `every 1 s`, `every 100 ms`, `every interval` (a `duration` parameter),
  `every day at 09:30` (a bare unit may take `at HH:MM`), or `continuously`.
  Units: `ms s min hour day week month yr` (plural allowed).
* `state NAME: goto TARGET [on condition], ... { actions }`.
* **Execution model.** A process wakes up every `every <interval>` (and when a
  signal it `listen`s to changes). On each wake-up it runs the code of its
  *current* state: first the action block, then the `goto`s **in order**. The
  first `goto` that has no guard, or whose guard is true, changes the state; the
  new state runs on the *next* wake-up. If no `goto` fires the process stays in
  the state. A state with no `goto` therefore never changes state.
* Consequence: **an action block runs on every wake-up while the process is in
  that state**, not only once on entry. A state such as
  `state BUY: goto HOLD on pos > 0 { pos = account.buy(...); }` buys again on each
  wake-up until the guard becomes true. Prefer an unguarded `goto` after a
  one-shot action (as `BUY` does in the first example), or guard on something the
  action changes.
* Guards are plain boolean expressions, evaluated each wake-up (they are *not*
  edge-triggered). `state NAME;` is a state with no actions and no transitions.
* The first state passed to `launch` is where the process starts. The name
  `INIT` is just a convention; any identifier works.

## Expressions and statements

* Literals: `42`, `0.05`, `true`, `"text"`, durations like `5 min`, `2_days`.
* Operators: `+ - * / % << >> & ^ | == != < <= > >= && ||`, unary `! -`,
  conditional `a ? b : c`, member access `.`, scope `::`, indexing `[ ]`
  (`Tickers[sym]`, `ticker[-1 day]` for history).
* `let x: float = 1.0;` `let y = 2;` `static n = 0;` `const k = 3;`
* `if cond { ... } else { ... }`, `while cond { ... }`, `loop { ... }`,
  `for i in 0..5 { ... }` (`0..=5` includes the end).
* `return expr;` inside events and functions.

## Events (reusable conditions)

An `event` is a function over signals that returns `true` when something
happened. Use it in `goto ... on`.

```sml
event const_positive(const signal ticker: Ticker, percent: float) {
    for i in 0..2 {
        if ticker[-i days] < ticker[-(i+1) days] {
            return false;
        }
    }
    return true;
}

// used as:  state INIT: goto ENTER on const_positive(Tickers[sym], change);
```

## Built-in library (`package/libs/std.sm`)

| Name | Meaning |
| --- | --- |
| `Tickers[sym]` | Live `signal Ticker` for a symbol (`HashMap<Symbol, signal Ticker>`). Has price fields such as `close` and `history(duration)`. |
| `Ticker` | One price series. `ticker[-1 day]` indexes back in time. |
| `Account` | `account.buy(sym, shares)` and `account.sell(sym, shares)` take a **number of shares** (not money; divide an amount by the price, e.g. `invest / Tickers[sym].close`) and return the `Position`. `sell` never sells more than is held. |
| `Order` and the extended account API | `buy_limit`, `sell_limit`, `buy_stop`, `sell_stop`, `cancel`, `cancel_all`, `cash()`, `equity()`, `bid/ask/last(sym)` and `order.status()/is_filled()/...`, see [brokers.md](brokers.md#orders-in-sml). **Studio's runtime only**: the C++ runtime has just `buy`/`sell`, so a strategy using these will not compile for it. |
| `SandBox` | In the C++ runtime: an in-memory simulation that tracks share counts, without cash, fills or fees. In Studio it is the account name for the simulated account, and a deployment can send the same calls to a brokerage connection instead ([brokers.md](brokers.md)). |
| `Position` | Holdings. `pos.shares` is the share count; `pos.value()` is meant to be `close * shares` at the latest price. **Known bug:** the C++ runtime never sets the position's ticker, so `value()` currently returns 0 there (the Studio simulator computes it correctly). Prefer `Tickers[sym].close * pos.shares` until this is fixed. |
| `Symbol`, `Vec<T>`, `HashMap<K,V>`, `duration` | Basic types. |
| `printf`, `sleep`, `std::rand`, `std::srand`, `std::time` | Utilities. |

The library is deliberately small. Read `package/libs/std.sm` to see exactly
what is available before writing a strategy; do not invent functions.

## Inline C++

`{% ... %}` embeds raw C++ in a block. It is compiled and run with the
privileges of the job server, so **treat any strategy containing it as code,
not configuration**. `ezt-agent` refuses to produce it unless you pass
`--allow-cpp`.

## Examples

* [`examples/threshold.sm`](../examples/threshold.sm) — buy below a floor, sell
  above a ceiling.
* [`examples/momentum.sm`](../examples/momentum.sm) — buy after two up days; sell
  on a profit target or a stop loss. Uses an `event`, history indexing
  (`ticker[-1 day]`) and several guards.
* [`tests/example.sm`](../tests/example.sm) — trailing stop with profit target
  and a heartbeat process.

Both `examples/` strategies come with a `.test.json` file and can be opened in
[Strategy Studio](studio.md), which draws the state machine, simulates the
strategy on price data and runs the tests.
