# Brokerage connections and orders

Strategy Studio can send a strategy's orders to a brokerage instead of its
built-in simulated account. A connection is a small JSON file; no code is needed
for a brokerage with a conventional HTTP API.

![A deployment trading through a brokerage connection](img/studio-broker-live.png)

> **Read this first.**
>
> * Everything here was tested against **local imitations** of two brokerage APIs
>   (`ezt-fakebroker`), never against a real brokerage. The shipped `alpaca-paper`
>   and `tradier-sandbox` descriptions were written from those services' public
>   documentation and are **unverified**. Use the **Test** button, then the paper
>   or sandbox environment, before trusting any connection.
> * Real-money connections are off by default and need four separate things (see
>   [Real money](#real-money)).
> * The order API in SML (`buy_limit`, `cancel`, ...) runs in **Studio's runtime**.
>   The C++ runtime's `SandBox` still only has `buy`/`sell`; a strategy that uses
>   the new calls will not compile for it.
> * You are responsible for what your strategy does. These controls limit the
>   damage of a bug; they do not make a strategy profitable or safe.

## Quick try, no account needed

```sh
cargo run -p broker --bin ezt-fakebroker -- --flavor alpaca --key demo --price ACME=100 &
mkdir -p strategies/brokers
sed -e 's#https://paper-api.alpaca.markets#http://127.0.0.1:18900#' \
    -e 's#https://data.alpaca.markets##' brokers/alpaca-paper.json > strategies/brokers/alpaca-paper.json
ALPACA_KEY_ID=demo ALPACA_SECRET_KEY=x cargo run -p studio
```

Open **Live → Brokerage connections → Test**, deploy a strategy with *Orders go
to: demo*, and move the market:

```sh
curl -X POST localhost:18900/__control/price -d '{"symbol":"ACME","price":90}'
curl -X POST localhost:18900/__control/bar   -d '{"symbol":"ACME","open":90,"high":95,"low":88,"close":94}'
curl -X POST localhost:18900/__control/fail  -d '{"count":3}'      # next 3 requests answer 503
```

## Orders in SML

On the account (`account` in the examples, the `SandBox` parameter):

| Call | Meaning |
| --- | --- |
| `account.buy(sym, shares)` / `account.sell(sym, shares)` | Market order. Returns the `Position`. A buy is reduced to what cash and `max_position_value` allow; a sell to what *this strategy* holds. |
| `account.buy_limit(sym, shares, price)` / `sell_limit` | Resting order; returns an `Order`. |
| `account.buy_stop(sym, shares, price)` / `sell_stop` | Becomes a market order when the price trades through `price` (a stop loss is a `sell_stop`). Returns an `Order`. |
| `account.cancel(order)` / `account.cancel_all()` | Cancel one or every working order (`cancel_all` returns how many). |
| `account.cash()`, `equity()`, `buying_power()` | The account's numbers (the brokerage's, for a connection). |
| `account.bid(sym)`, `ask(sym)`, `last(sym)` | A quote from the broker. |
| `account.position(sym)` | What **this strategy** bought and still holds. |
| `account.open_orders()` | List of the strategy's working orders. |
| `order.status()` | `open`, `partially_filled`, `filled`, `canceled`, `rejected` or `expired`. |
| `order.is_open()`, `is_filled()`, `filled()`, `price()`, `remaining()`, `cancel()` | Inspect or cancel. |

Order state is refreshed **at the start of each wake-up**, so a strategy sees a
fill on the next bar after it happens. See [`examples/bracket.sm`](../examples/bracket.sm)
for a limit entry with a stop-loss and a profit target.

Rules worth knowing:

* A refusal by the brokerage or by a safety limit is **not an error of the
  strategy**: you get an `Order` whose status is `rejected` (with the reason in
  the activity log) and the strategy carries on.
* An outage, bad credentials or a nonsensical reply **stops the deployment**
  (status *error*). The wake-up may have been half done, so check the account
  before deploying again.
* A strategy only sees and sells what it bought itself, so several deployments can
  share a brokerage account without selling each other's positions. Positions the
  account held before are not visible to strategies (the deploy log says when
  there are some).
* Two exits placed together (a stop and a target, as in the example) are **not**
  linked as OCO orders: if both trigger on the same bar the second one is
  refused because nothing is left to sell.
* Backtests and simulated deployments use the same calls against a simulated
  broker: market orders fill at the bar's close; limit and stop orders rest and
  fill when a later bar's range reaches them (at the limit/stop price, or at the
  open if the bar gapped past it). "Day" orders are treated as good-till-cancelled.

## A connection file

Files live in `brokers/<name>.json` in the Studio workspace; the file name must
equal `name`. The web UI can read them but not write them (a page that could add
a connection could point the server at any URL). Example (abridged from
[`brokers/alpaca-paper.json`](../brokers/alpaca-paper.json)):

```json
{
  "name": "alpaca-paper",
  "environment": "paper",
  "base_url": "https://paper-api.alpaca.markets",
  "headers": { "APCA-API-KEY-ID": "${ALPACA_KEY_ID}", "APCA-API-SECRET-KEY": "${ALPACA_SECRET_KEY}" },
  "limits": { "max_order_value": 2000, "max_position_value": 5000, "max_orders": 200 },

  "account":    { "path": "/v2/account", "map": { "cash": "cash", "equity": "equity", "buying_power": "buying_power" } },
  "positions":  { "path": "/v2/positions", "map": { "symbol": "symbol", "qty": "qty", "avg_cost": "avg_entry_price" } },
  "quote":      { "path": "https://data.alpaca.markets/v2/stocks/{symbol}/quotes/latest", "at": "quote", "map": { "bid": "bp", "ask": "ap" } },
  "place_order": {
    "path": "/v2/orders",
    "market": { "symbol": "{symbol}", "qty": "{qty}", "side": "{side}", "type": "market", "time_in_force": "{tif}", "client_order_id": "{client_id}" },
    "limit":  { "...": "same plus \"limit_price\": \"{limit}\"" },
    "stop":   { "...": "same plus \"stop_price\": \"{stop}\"" }
  },
  "cancel_order": { "method": "DELETE", "path": "/v2/orders/{id}" },
  "order":        { "path": "/v2/orders/{id}" },
  "open_orders":  { "path": "/v2/orders", "query": { "status": "open" } },
  "order_by_client_id": { "path": "/v2/orders:by_client_order_id", "query": { "client_order_id": "{client_id}" } },
  "order_map": { "id": "id", "client_id": "client_order_id", "symbol": "symbol", "side": "side", "qty": "qty",
                 "filled_qty": "filled_qty", "avg_price": "filled_avg_price", "state": "status" },
  "states": { "new": "open", "filled": "filled", "canceled": "canceled", "rejected": "rejected" }
}
```

* **`environment`**: `paper` (sandbox money) or `live` (real money).
* **Secrets** are `${NAME}` placeholders read from the server's environment when a
  connection is made. Never put a key in the file. Values are redacted from every
  error message. Studio shows which variables are missing.
* **Paths** are joined to `base_url`, or are absolute URLs (quotes often live on a
  different host). `{symbol}`, `{id}` and `{client_id}` are filled in (URL-encoded).
* **`at`** says where the useful object (or list) is in the reply; **`map`** maps
  our field names to paths inside it. Paths are dotted with `[n]` indices;
  `a.b|c.d` takes the first that is present. Numbers may be JSON numbers or
  numeric strings. A list may be an array, a single object or the string
  `"null"`, since some APIs answer all three.
* **Order bodies**: one template per order kind. `"{qty:number}"` (the whole
  string) becomes a JSON number. `"body_format": "form"` sends form-encoded data
  (see [`brokers/tradier-sandbox.json`](../brokers/tradier-sandbox.json)).
  `"refetch": true` reads the order back after placing it, for APIs that answer
  with just an id.
* **`states`** maps the brokerage's status words (lower-cased) to ours.
* **`vocab`** renames `buy`, `sell`, `day`, `gtc` if the API uses other words.
* **`order_by_client_id`** (optional): lets the adapter recover when a placement
  timed out or looks like a duplicate, by asking whether the brokerage already has
  that client id. Without it, a timeout is reported as "may or may not have been
  placed" and is never retried blindly.

Not supported by the description format: OAuth flows that need a browser or token
refresh, websockets/streaming, FIX, multi-leg and options orders, fractional
price increments or exchange-specific rules. For those, implement the `Broker`
trait in `core/broker` (about eight methods).

## Safety

Several layers, all enforced by the server outside the strategy:

| Layer | What it does |
| --- | --- |
| Connection `limits` + the Deploy dialog | `max_order_value`, `max_position_value`, `max_orders`, `symbols`. Both apply and the **stricter** wins; a deployment can tighten a connection's limits but never loosen them. A breach is refused before any request is sent. |
| Kill switch | Global. While on, no deployment can **buy**; selling and cancelling still work so positions can be closed. |
| Emergency stop | Kill switch on, every brokerage deployment paused, its working orders cancelled. Positions stay open; use **Flatten** per deployment to close them. Resuming is refused until the kill switch is turned off. |
| Max drawdown | Closes positions and pauses (as for any deployment). |
| Sizing | Buys are cut to the account's buying power; sells to what the strategy holds (no accidental shorting). |
| No silent retries | Only reads are retried (once). An order is never re-sent unless the brokerage confirms it does not have it. |

Limits apply per run of the server: after a restart `max_orders` counts from
zero again.

### Real money

A connection with `"environment": "live"` can only be used when **all** of these
hold:

1. the server was started with `--allow-live`;
2. the connection has both `max_order_value` and `max_position_value` (its own or
   from the deployment);
3. the deployment uses a real price feed (a broker or Twelve Data feed), not a
   simulated or replayed one;
4. you type the connection's name in the Deploy dialog to confirm.

A live deployment is **not resumed automatically** after a server restart: it
comes back paused and says so. Do not run Studio on a network you do not
control: it has no login (see [studio.md](studio.md#security)).

## What happens on a restart

Orders are placed with a client id (`<deployment>-<n>`). Every call to the
brokerage and its answer is written to the deployment's `log.jsonl` next to the
bars. After a restart the engine is rebuilt by replaying the bars with those
**recorded answers**, so nothing is sent again and the strategy is back in the
same state with the same positions and orders. If the log ends in the middle of a
wake-up (a crash between sending an order and writing its answer), the unrecorded
call is made for real with the same client id, and the adapter asks the
brokerage whether it already has it. If the strategy now asks for something
different from what was recorded the deployment stops with "restart recovery
diverged" rather than guess. If the credentials are missing after a restart, the
deployment is restored (state from the log) but stays paused, saying why.

## API

| Request | Purpose |
| --- | --- |
| `GET /api/brokers` | Connections, readiness, missing variables, `kill_switch`, `allow_live` |
| `POST /api/brokers/{name}/test` `{"symbol"?}` | Read-only check: credentials, account, positions, open orders, a quote. Never places an order. |
| `POST /api/brokers/kill` `{"on": true\|false}` | Kill switch |
| `POST /api/live/emergency-stop` | Kill switch on, pause and cancel as above |
| `POST /api/live` with `"execution": {"kind":"broker","broker":"name","limits":{...},"confirm":"name"}` | Deploy through a connection; `feed` may be `{"kind":"broker","broker":"name","symbols":[...],"tick_ms":60000}` |
| `POST /api/live/{id}/cancel-orders` | Cancel the deployment's working orders |

## Writing an adapter in Rust

Implement `ezt_broker::Broker` (`account`, `positions`, `quote`, `place_order`,
`cancel_order`, `order`, `open_orders`; `on_bar` for simulated brokers). Wrap it
in `Guarded`, `Recording` and `Replay` exactly as `Brokers::trading` does in
`core/studio/src/brokers.rs` to get limits and restart recovery.
`PaperBroker` is the reference implementation, and `ezt_broker::fake` is a
stateful fake brokerage you can test against.

## What was tested

* Unit and integration tests for the paper broker (market/limit/stop, gaps, cash
  and holdings limits, idempotent client ids), the guard, record/replay and
  divergence detection, and the REST adapter against two fake brokerages with
  different dialects (errors vs outages, lost replies, wrong credentials, secret
  redaction, single-object/`"null"` lists).
* Studio tests: orders reaching a broker, strategy trading through a brokerage,
  restart recovery without re-sending, a crash between sending and logging an
  order, outages, missing credentials after a restart, limits, the kill switch and
  emergency stop, the live-money gate, and the HTTP API.
* A browser run against `ezt-fakebroker`: test button, deploy dialog, orders and
  fills, cancel, kill switch and emergency stop.

**Not tested:** any real brokerage, real money, market-hours behaviour,
rate limits, partial fills from a real venue, long soak runs.
