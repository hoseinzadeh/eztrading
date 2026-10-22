//! Stand-in brokerage servers for tests, demos and `ezt-fakebroker`.
//!
//! They speak two deliberately different dialects so that the declarative
//! adapter is exercised against more than one shape:
//!
//! * [`Flavor::Alpaca`]: JSON, numbers as strings, auth by key headers, `422` on a repeated client id;
//! * [`Flavor::Tradier`]: form-encoded orders, nested `{"orders":{"order": ...}}` replies that are an
//!   object when there is one item and the string `"null"` when there are none.
//!
//! These are *imitations written from the public API descriptions*, not the real services.
//! Control endpoints (`POST /__control/...`) move the market and inject failures.

use crate::*;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Flavor {
    Alpaca,
    Tradier,
}

struct Fake {
    flavor: Flavor,
    key: String,
    account: String,
    paper: PaperBroker,
    /// Answer the next N requests with 503.
    fail_next: u32,
    /// Process the next order placement, but reply 503 (the reply is "lost").
    lose_reply: bool,
    log: Vec<String>,
}

pub struct FakeServer {
    pub addr: SocketAddr,
    stop: Arc<AtomicBool>,
    state: Arc<Mutex<Fake>>,
}

impl FakeServer {
    pub fn start(flavor: Flavor, key: &str, account: &str, cash: f64) -> FakeServer {
        Self::start_on("127.0.0.1:0", flavor, key, account, cash)
    }

    pub fn start_on(bind: &str, flavor: Flavor, key: &str, account: &str, cash: f64) -> FakeServer {
        let listener = TcpListener::bind(bind).expect("bind");
        let addr = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let state = Arc::new(Mutex::new(Fake {
            flavor,
            key: key.into(),
            account: account.into(),
            paper: PaperBroker::new(cash, 0.0),
            fail_next: 0,
            lose_reply: false,
            log: Vec::new(),
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let (st, sp) = (state.clone(), stop.clone());
        std::thread::spawn(move || {
            while !sp.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((s, _)) => {
                        let st = st.clone();
                        std::thread::spawn(move || {
                            let _ = handle(s, st);
                        });
                    }
                    Err(_) => std::thread::sleep(std::time::Duration::from_millis(5)),
                }
            }
        });
        FakeServer { addr, stop, state }
    }

    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Move the market: a bar for `symbol` (resting orders trade against it).
    pub fn bar(&self, symbol: &str, open: f64, high: f64, low: f64, close: f64) {
        let mut f = self.state.lock().unwrap();
        f.paper.on_bar(&BTreeMap::from([(symbol.to_owned(), Candle { open, high, low, close })]));
    }

    pub fn price(&self, symbol: &str, price: f64) {
        self.state.lock().unwrap().paper.set_price(symbol, price);
    }

    pub fn fail_next(&self, n: u32) {
        self.state.lock().unwrap().fail_next = n;
    }

    pub fn lose_next_reply(&self) {
        self.state.lock().unwrap().lose_reply = true;
    }

    /// `METHOD /path` of every brokerage request so far.
    pub fn requests(&self) -> Vec<String> {
        self.state.lock().unwrap().log.clone()
    }

    /// What the brokerage itself holds (to compare with what a strategy believes).
    pub fn positions(&self) -> Vec<Position> {
        self.state.lock().unwrap().paper.positions().unwrap()
    }

    pub fn cash(&self) -> f64 {
        self.state.lock().unwrap().paper.account().unwrap().cash
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

impl Drop for FakeServer {
    fn drop(&mut self) {
        self.stop();
    }
}

struct Req {
    method: String,
    path: String,
    query: BTreeMap<String, String>,
    headers: BTreeMap<String, String>,
    body: String,
}

fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => {
                let h = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok());
                match h {
                    Some(v) => {
                        out.push(v);
                        i += 2;
                    }
                    None => out.push(b'%'),
                }
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn pairs(s: &str) -> BTreeMap<String, String> {
    s.split('&').filter(|p| !p.is_empty()).map(|p| {
        let (k, v) = p.split_once('=').unwrap_or((p, ""));
        (unescape(k), unescape(v))
    }).collect()
}

fn handle(mut s: TcpStream, st: Arc<Mutex<Fake>>) -> std::io::Result<()> {
    s.set_nonblocking(false)?;
    s.set_read_timeout(Some(std::time::Duration::from_secs(5)))?;
    let mut r = BufReader::new(s.try_clone()?);
    let mut line = String::new();
    r.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let (method, target) = (parts.next().unwrap_or("").to_owned(), parts.next().unwrap_or("/").to_owned());
    let mut headers = BTreeMap::new();
    loop {
        let mut h = String::new();
        r.read_line(&mut h)?;
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.insert(k.trim().to_lowercase(), v.trim().to_owned());
        }
    }
    let len: usize = headers.get("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
    let mut body = vec![0u8; len];
    r.read_exact(&mut body)?;
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p.to_owned(), pairs(q)),
        None => (target.clone(), BTreeMap::new()),
    };
    let req = Req { method, path, query, headers, body: String::from_utf8_lossy(&body).into_owned() };
    let (code, reply) = {
        let mut f = st.lock().unwrap();
        route(&mut f, &req)
    };
    let text = if reply.is_null() { String::new() } else { reply.to_string() };
    write!(s, "HTTP/1.1 {code} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}", text.len())?;
    Ok(())
}

fn n(x: f64) -> Value {
    json!(x)
}

fn route(f: &mut Fake, r: &Req) -> (u16, Value) {
    if let Some(c) = r.path.strip_prefix("/__control/") {
        let b: Value = serde_json::from_str(&r.body).unwrap_or(Value::Null);
        let g = |k: &str| b.get(k).and_then(Value::as_f64).unwrap_or(0.0);
        let sym = b.get("symbol").and_then(Value::as_str).unwrap_or("").to_owned();
        match c {
            "bar" => f.paper.on_bar(&BTreeMap::from([(sym, Candle { open: g("open"), high: g("high"), low: g("low"), close: g("close") })])),
            "price" => f.paper.set_price(&sym, g("price")),
            "fail" => f.fail_next = g("count") as u32,
            _ => return (404, json!({"message": "unknown control"})),
        }
        return (200, json!({"ok": true}));
    }
    f.log.push(format!("{} {}", r.method, r.path));
    let authed = match f.flavor {
        Flavor::Alpaca => r.headers.get("apca-api-key-id") == Some(&f.key),
        Flavor::Tradier => r.headers.get("authorization") == Some(&format!("Bearer {}", f.key)),
    };
    if !authed {
        return (401, json!({"message": "unauthorized"}));
    }
    if f.fail_next > 0 {
        f.fail_next -= 1;
        return (503, json!({"message": "service unavailable"}));
    }
    match f.flavor {
        Flavor::Alpaca => alpaca(f, r),
        Flavor::Tradier => tradier(f, r),
    }
}

fn status_word(f: Flavor, o: &Order) -> &'static str {
    match (f, o.state) {
        (Flavor::Alpaca, OrderState::Open) => "new",
        (Flavor::Tradier, OrderState::Open) => "open",
        (_, s) => s.as_str(),
    }
}

fn alpaca_order(o: &Order) -> Value {
    let mut v = json!({
        "id": o.id, "client_order_id": o.client_id, "symbol": o.symbol, "side": o.side.as_str(),
        "qty": o.qty.to_string(), "filled_qty": o.filled_qty.to_string(),
        "filled_avg_price": if o.filled_qty > 0.0 { json!(o.avg_price.to_string()) } else { Value::Null },
        "status": status_word(Flavor::Alpaca, o),
    });
    if !o.message.is_empty() {
        v["message"] = json!(o.message);
    }
    v
}

/// Real brokerages refuse an order they cannot fund instead of part-filling it.
fn unfunded(f: &mut Fake, r: &OrderRequest) -> bool {
    if r.side != Side::Buy {
        return false;
    }
    let price = match r.kind {
        Kind::Limit { limit } => limit,
        Kind::Stop { stop } => stop,
        Kind::Market => f.paper.quote(&r.symbol).map_or(0.0, |q| q.ask),
    };
    r.qty * price > f.paper.account().map_or(0.0, |a| a.buying_power) + 1e-9
}

fn lose(f: &mut Fake) -> bool {
    std::mem::take(&mut f.lose_reply)
}

fn alpaca(f: &mut Fake, r: &Req) -> (u16, Value) {
    let p = r.path.as_str();
    match (r.method.as_str(), p) {
        ("GET", "/v2/account") => {
            let a = f.paper.account().unwrap();
            (200, json!({"cash": a.cash.to_string(), "equity": a.equity.to_string(), "buying_power": a.buying_power.to_string(), "currency": "USD"}))
        }
        ("GET", "/v2/positions") => (
            200,
            Value::Array(f.paper.positions().unwrap().iter().map(|p| json!({"symbol": p.symbol, "qty": p.qty.to_string(), "avg_entry_price": p.avg_cost.to_string(), "market_value": p.market_value.to_string()})).collect()),
        ),
        ("GET", "/v2/orders") => (200, Value::Array(f.paper.open_orders().unwrap().iter().map(alpaca_order).collect())),
        ("GET", "/v2/orders:by_client_order_id") => {
            let id = r.query.get("client_order_id").cloned().unwrap_or_default();
            // the paper broker dedupes by client id, so asking for a never-seen id would create nothing
            match f.paper.open_orders().unwrap().into_iter().chain(all_orders(f)).find(|o| o.client_id == id) {
                Some(o) => (200, alpaca_order(&o)),
                None => (404, json!({"message": "order not found"})),
            }
        }
        ("POST", "/v2/orders") => {
            let b: Value = serde_json::from_str(&r.body).unwrap_or(Value::Null);
            let client_id = b["client_order_id"].as_str().unwrap_or("").to_owned();
            if all_orders(f).iter().any(|o| o.client_id == client_id) {
                return (422, json!({"message": "client_order_id must be unique"}));
            }
            let s = |k: &str| b[k].as_str().unwrap_or("").to_owned();
            let num = |k: &str| b[k].as_str().and_then(|x| x.parse::<f64>().ok());
            let kind = match s("type").as_str() {
                "market" => Kind::Market,
                "limit" => Kind::Limit { limit: num("limit_price").unwrap_or(0.0) },
                "stop" => Kind::Stop { stop: num("stop_price").unwrap_or(0.0) },
                other => return (422, json!({"message": format!("unsupported order type {other}")})),
            };
            let req = OrderRequest {
                symbol: s("symbol"),
                side: if s("side") == "sell" { Side::Sell } else { Side::Buy },
                qty: num("qty").unwrap_or(0.0),
                kind,
                tif: if s("time_in_force") == "gtc" { Tif::Gtc } else { Tif::Day },
                client_id,
            };
            if unfunded(f, &req) {
                return (403, json!({"message": "insufficient buying power"}));
            }
            match f.paper.place_order(&req) {
                Ok(o) if o.state == OrderState::Rejected => (403, json!({"message": format!("order rejected: {}", o.message)})),
                Ok(o) => {
                    if lose(f) {
                        return (503, json!({"message": "gateway timeout"}));
                    }
                    (200, alpaca_order(&o))
                }
                Err(e) => (422, json!({"message": e.message})),
            }
        }
        ("GET", _) if p.starts_with("/v2/stocks/") && p.ends_with("/quotes/latest") => {
            let sym = p.trim_start_matches("/v2/stocks/").trim_end_matches("/quotes/latest");
            match f.paper.quote(sym) {
                Ok(q) => (200, json!({"symbol": sym, "quote": {"bp": n(q.bid - 0.01), "ap": n(q.ask + 0.01)}})),
                Err(e) => (404, json!({"message": e.message})),
            }
        }
        ("GET", _) if p.starts_with("/v2/orders/") => match f.paper.order(p.trim_start_matches("/v2/orders/")) {
            Ok(o) => (200, alpaca_order(&o)),
            Err(_) => (404, json!({"message": "order not found"})),
        },
        ("DELETE", _) if p.starts_with("/v2/orders/") => match f.paper.cancel_order(p.trim_start_matches("/v2/orders/")) {
            Ok(_) => (204, Value::Null),
            Err(_) => (404, json!({"message": "order not found"})),
        },
        _ => (404, json!({"message": "not found"})),
    }
}

fn all_orders(f: &mut Fake) -> Vec<Order> {
    // refused orders are not stored by a real brokerage
    (1..=10_000).map_while(|i| f.paper.order(&format!("P{i}")).ok()).filter(|o| o.state != OrderState::Rejected).collect()
}

fn tradier_order(o: &Order) -> Value {
    json!({
        "id": o.id.trim_start_matches('P').parse::<u64>().unwrap_or(0), "type": o.kind.describe(), "symbol": o.symbol, "side": o.side.as_str(),
        "quantity": o.qty, "status": status_word(Flavor::Tradier, o), "avg_fill_price": o.avg_price, "exec_quantity": o.filled_qty, "tag": o.client_id,
    })
}

fn tradier(f: &mut Fake, r: &Req) -> (u16, Value) {
    let base = format!("/v1/accounts/{}", f.account);
    let Some(rest) = r.path.strip_prefix(&base).map(str::to_owned).or_else(|| r.path.starts_with("/v1/markets/").then(|| r.path.clone())) else {
        return (404, json!({"fault": {"faultstring": "unknown account"}}));
    };
    match (r.method.as_str(), rest.as_str()) {
        ("GET", "/balances") => {
            let a = f.paper.account().unwrap();
            (200, json!({"balances": {"total_cash": a.cash, "total_equity": a.equity, "market_value": a.equity - a.cash}}))
        }
        ("GET", "/positions") => {
            let ps = f.paper.positions().unwrap();
            let items: Vec<Value> = ps.iter().map(|p| json!({"symbol": p.symbol, "quantity": p.qty, "cost_basis": p.qty * p.avg_cost})).collect();
            match items.len() {
                0 => (200, json!({"positions": "null"})),
                1 => (200, json!({"positions": {"position": items[0]}})),
                _ => (200, json!({"positions": {"position": items}})),
            }
        }
        ("GET", "/orders") => {
            let items: Vec<Value> = all_orders(f).iter().filter(|o| !o.state.is_done()).map(tradier_order).collect();
            match items.len() {
                0 => (200, json!({"orders": "null"})),
                1 => (200, json!({"orders": {"order": items[0]}})),
                _ => (200, json!({"orders": {"order": items}})),
            }
        }
        ("GET", "/v1/markets/quotes") => {
            let sym = r.query.get("symbols").cloned().unwrap_or_default();
            match f.paper.quote(&sym) {
                Ok(q) => (200, json!({"quotes": {"quote": {"symbol": sym, "last": q.last, "bid": q.bid - 0.01, "ask": q.ask + 0.01}}})),
                Err(_) => (200, json!({"quotes": {"unmatched_symbols": {"symbol": sym}}})),
            }
        }
        ("POST", "/orders") => {
            let b = pairs(&r.body);
            let g = |k: &str| b.get(k).cloned().unwrap_or_default();
            let num = |k: &str| b.get(k).and_then(|x| x.parse::<f64>().ok());
            let kind = match g("type").as_str() {
                "market" => Kind::Market,
                "limit" => Kind::Limit { limit: num("price").unwrap_or(0.0) },
                "stop" => Kind::Stop { stop: num("stop").unwrap_or(0.0) },
                other => return (400, json!({"errors": {"error": [format!("unsupported type {other}")]}})),
            };
            let req = OrderRequest {
                symbol: g("symbol"),
                side: if g("side") == "sell" { Side::Sell } else { Side::Buy },
                qty: num("quantity").unwrap_or(0.0),
                kind,
                tif: if g("duration") == "gtc" { Tif::Gtc } else { Tif::Day },
                client_id: g("tag"),
            };
            if unfunded(f, &req) {
                return (400, json!({"errors": {"error": ["insufficient funds"]}}));
            }
            match f.paper.place_order(&req) {
                Ok(o) if o.state == OrderState::Rejected => (400, json!({"errors": {"error": [format!("rejected: {}", o.message)]}})),
                Ok(o) => {
                    if lose(f) {
                        return (503, json!({"fault": {"faultstring": "timeout"}}));
                    }
                    (200, json!({"order": {"id": o.id.trim_start_matches('P').parse::<u64>().unwrap_or(0), "status": "ok"}}))
                }
                Err(e) => (400, json!({"errors": {"error": [e.message]}})),
            }
        }
        ("GET", _) if rest.starts_with("/orders/") => match f.paper.order(&format!("P{}", rest.trim_start_matches("/orders/"))) {
            Ok(o) => (200, json!({"order": tradier_order(&o)})),
            Err(_) => (404, json!({"errors": {"error": ["order not found"]}})),
        },
        ("DELETE", _) if rest.starts_with("/orders/") => match f.paper.cancel_order(&format!("P{}", rest.trim_start_matches("/orders/"))) {
            Ok(_) => (200, json!({"order": {"status": "ok"}})),
            Err(_) => (404, json!({"errors": {"error": ["order not found"]}})),
        },
        _ => (404, json!({"fault": {"faultstring": "not found"}})),
    }
}
