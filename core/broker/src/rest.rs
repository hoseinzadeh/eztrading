//! Connect any HTTP brokerage by describing it in JSON.
//!
//! A [`Spec`] says where each operation lives (`GET /v2/account`...), how to
//! build the request body and which fields of the reply mean what. No code is
//! needed for brokerages with a conventional REST API; see `docs/brokers.md`.
//!
//! Secrets never appear in a spec: write `${NAME}` and the value is read from
//! the environment when the connection is made, and redacted from every error.

use crate::*;
use reqwest::blocking::Client;
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::time::Duration;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub environment: Environment,
    pub base_url: String,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    /// Safety limits applied to every strategy that trades through this connection.
    #[serde(default)]
    pub limits: Limits,
    pub account: Endpoint,
    pub positions: Endpoint,
    pub quote: Endpoint,
    pub place_order: PlaceEndpoint,
    pub cancel_order: Endpoint,
    pub order: Endpoint,
    pub open_orders: Endpoint,
    /// Optional: look an order up by our client id (used to recover when a
    /// placement timed out and its outcome is unknown).
    #[serde(default)]
    pub order_by_client_id: Option<Endpoint>,
    pub order_map: OrderMap,
    /// Brokerage status text (lower-cased) -> `open | partially_filled | filled | canceled | rejected | expired`.
    pub states: BTreeMap<String, OrderState>,
    #[serde(default)]
    pub vocab: Vocab,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Endpoint {
    #[serde(default = "get")]
    pub method: String,
    /// Path (joined to `base_url`) or an absolute URL. `{symbol}`, `{id}` and `{client_id}` are filled in.
    pub path: String,
    #[serde(default)]
    pub query: BTreeMap<String, String>,
    #[serde(default)]
    pub body: Option<Value>,
    /// Where the useful object is (or the array, for lists). Default: the whole reply.
    #[serde(default)]
    pub at: Option<String>,
    /// Field name -> path inside the reply.
    #[serde(default)]
    pub map: BTreeMap<String, String>,
}

fn get() -> String {
    "GET".into()
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PlaceEndpoint {
    #[serde(default = "post")]
    pub method: String,
    pub path: String,
    #[serde(default)]
    pub body_format: BodyFormat,
    /// One body per order kind. Placeholders: `{symbol} {side} {qty} {tif} {client_id} {limit} {stop}`.
    /// A string that is exactly `{qty:number}` becomes a JSON number.
    pub market: Value,
    pub limit: Value,
    pub stop: Value,
    /// Where the order object is in the reply (default: the whole reply).
    #[serde(default)]
    pub at: Option<String>,
    /// Some brokerages answer a placement with just an id; fetch the order afterwards to see its real state.
    #[serde(default)]
    pub refetch: bool,
}

fn post() -> String {
    "POST".into()
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, Default, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum BodyFormat {
    #[default]
    Json,
    Form,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
#[serde(deny_unknown_fields)]
pub struct OrderMap {
    pub id: String,
    #[serde(default)]
    pub client_id: Option<String>,
    #[serde(default)]
    pub symbol: Option<String>,
    #[serde(default)]
    pub side: Option<String>,
    #[serde(default)]
    pub qty: Option<String>,
    #[serde(default)]
    pub filled_qty: Option<String>,
    #[serde(default)]
    pub avg_price: Option<String>,
    #[serde(default)]
    pub fees: Option<String>,
    pub state: String,
    #[serde(default)]
    pub message: Option<String>,
}

/// The words this brokerage uses for our concepts.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Vocab {
    #[serde(default)]
    pub buy: Option<String>,
    #[serde(default)]
    pub sell: Option<String>,
    #[serde(default)]
    pub day: Option<String>,
    #[serde(default)]
    pub gtc: Option<String>,
}

impl Spec {
    /// Parse and check a spec (without reading any environment variables).
    pub fn parse(v: &Value) -> std::result::Result<Spec, String> {
        let spec: Spec = serde_json::from_value(v.clone()).map_err(|e| e.to_string())?;
        spec.check()?;
        Ok(spec)
    }

    fn check(&self) -> std::result::Result<(), String> {
        if self.name.trim().is_empty() || !self.name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            return Err("`name` must be letters, digits, `-` or `_`".into());
        }
        if !(self.base_url.starts_with("https://") || self.base_url.starts_with("http://")) {
            return Err("`base_url` must start with https:// or http://".into());
        }
        if self.base_url.starts_with("http://") && self.environment == Environment::Live {
            return Err("a live connection must use https".into());
        }
        for (what, e) in [("account", &self.account), ("positions", &self.positions), ("quote", &self.quote), ("cancel_order", &self.cancel_order), ("order", &self.order), ("open_orders", &self.open_orders)] {
            Method::from_bytes(e.method.to_uppercase().as_bytes()).map_err(|_| format!("`{what}.method` is not an HTTP method"))?;
        }
        for k in ["cash"] {
            if !self.account.map.contains_key(k) {
                return Err(format!("`account.map` needs `{k}`"));
            }
        }
        for k in ["symbol", "qty"] {
            if !self.positions.map.contains_key(k) {
                return Err(format!("`positions.map` needs `{k}`"));
            }
        }
        if !self.quote.map.contains_key("last") && !(self.quote.map.contains_key("bid") && self.quote.map.contains_key("ask")) {
            return Err("`quote.map` needs `last` (or both `bid` and `ask`)".into());
        }
        for needed in ["open", "filled"] {
            if !self.states.values().any(|s| s.as_str() == needed) {
                return Err(format!("`states` must map something to `{needed}`"));
            }
        }
        Ok(())
    }

    /// Names of `${NAME}` placeholders anywhere in the spec.
    pub fn env_names(&self) -> Vec<String> {
        let mut out = Vec::new();
        collect_env(&serde_json::to_value(self).unwrap_or(Value::Null), &mut out);
        out.sort();
        out.dedup();
        out
    }
}

fn collect_env(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) => {
            let mut rest = s.as_str();
            while let Some(i) = rest.find("${") {
                let after = &rest[i + 2..];
                let Some(j) = after.find('}') else { break };
                out.push(after[..j].to_owned());
                rest = &after[j + 1..];
            }
        }
        Value::Array(a) => a.iter().for_each(|x| collect_env(x, out)),
        Value::Object(o) => o.values().for_each(|x| collect_env(x, out)),
        _ => (),
    }
}

fn resolve_env(v: &Value, env: &dyn Fn(&str) -> Option<String>, secrets: &mut Vec<String>) -> std::result::Result<Value, String> {
    Ok(match v {
        Value::String(s) => {
            let mut out = String::new();
            let mut rest = s.as_str();
            while let Some(i) = rest.find("${") {
                out.push_str(&rest[..i]);
                let after = &rest[i + 2..];
                let Some(j) = after.find('}') else {
                    out.push_str(&rest[i..]);
                    rest = "";
                    break;
                };
                let name = &after[..j];
                let val = env(name).filter(|v| !v.is_empty()).ok_or_else(|| format!("the environment variable `{name}` is not set"))?;
                if val.len() >= 4 {
                    secrets.push(val.clone());
                }
                out.push_str(&val);
                rest = &after[j + 1..];
            }
            out.push_str(rest);
            Value::String(out)
        }
        Value::Array(a) => Value::Array(a.iter().map(|x| resolve_env(x, env, secrets)).collect::<std::result::Result<_, _>>()?),
        Value::Object(o) => Value::Object(o.iter().map(|(k, x)| Ok((k.clone(), resolve_env(x, env, secrets)?))).collect::<std::result::Result<Map<_, _>, String>>()?),
        other => other.clone(),
    })
}

// ---- reply navigation --------------------------------------------------

fn walk<'a>(v: &'a Value, path: &str) -> Option<&'a Value> {
    let mut cur = v;
    let path = path.trim();
    if path.is_empty() || path == "$" {
        return Some(cur);
    }
    for seg in path.trim_start_matches("$.").split('.') {
        let (name, mut idxs) = match seg.find('[') {
            Some(i) => (&seg[..i], &seg[i..]),
            None => (seg, ""),
        };
        if !name.is_empty() {
            cur = cur.get(name)?;
        }
        while let Some(rest) = idxs.strip_prefix('[') {
            let end = rest.find(']')?;
            let i: usize = rest[..end].parse().ok()?;
            cur = cur.get(i)?;
            idxs = &rest[end + 1..];
        }
    }
    Some(cur)
}

/// `a.b|c.d`: the first alternative that is present and not null.
fn pick<'a>(v: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('|').filter_map(|alt| walk(v, alt)).find(|x| !x.is_null())
}

fn number(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
    .filter(|x| x.is_finite())
}

fn text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn field_num(v: &Value, map: &BTreeMap<String, String>, key: &str) -> Option<f64> {
    map.get(key).and_then(|p| pick(v, p)).and_then(number)
}

/// An array at `path`; a lone object counts as a one-element list and
/// `null`/missing/"" as an empty one (some APIs do all three).
fn list_at(v: &Value, path: &Option<String>) -> Vec<Value> {
    let target = match path {
        Some(p) => pick(v, p),
        None => Some(v),
    };
    match target {
        Some(Value::Array(a)) => a.clone(),
        Some(o @ Value::Object(_)) => vec![o.clone()],
        _ => Vec::new(),
    }
}

fn render(template: &str, vars: &BTreeMap<&str, String>) -> std::result::Result<String, String> {
    let mut out = String::new();
    let mut rest = template;
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let Some(j) = after.find('}') else {
            return Err(format!("unclosed `{{` in `{template}`"));
        };
        let name = after[..j].trim_end_matches(":number");
        match vars.get(name) {
            Some(v) => out.push_str(v),
            None => return Err(format!("`{{{name}}}` is not available here (in `{template}`)")),
        }
        rest = &after[j + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

fn render_value(v: &Value, vars: &BTreeMap<&str, String>) -> std::result::Result<Value, String> {
    Ok(match v {
        Value::String(s) => {
            if s.starts_with('{') && s.ends_with(":number}") && s.matches('{').count() == 1 {
                let r = render(s, vars)?;
                let n: f64 = r.parse().map_err(|_| format!("`{r}` is not a number"))?;
                json!(n)
            } else {
                Value::String(render(s, vars)?)
            }
        }
        Value::Array(a) => Value::Array(a.iter().map(|x| render_value(x, vars)).collect::<std::result::Result<_, _>>()?),
        Value::Object(o) => Value::Object(o.iter().map(|(k, x)| Ok((k.clone(), render_value(x, vars)?))).collect::<std::result::Result<Map<_, _>, String>>()?),
        other => other.clone(),
    })
}

fn encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn num_text(x: f64) -> String {
    format!("{x}")
}

// ---- the broker ---------------------------------------------------------

pub struct RestBroker {
    spec: Spec,
    client: Option<Client>,
    secrets: Vec<String>,
    headers: BTreeMap<String, String>,
    base: String,
}

type Vars<'a> = BTreeMap<&'a str, String>;

impl RestBroker {
    /// Build a connection; `env` supplies the `${NAME}` placeholders.
    pub fn connect(spec: &Value, env: &dyn Fn(&str) -> Option<String>) -> Result<RestBroker> {
        let mut secrets = Vec::new();
        let resolved = resolve_env(spec, env, &mut secrets).map_err(|e| BrokerError::new(ErrorKind::Auth, e))?;
        let spec = Spec::parse(&resolved).map_err(|e| BrokerError::rejected(format!("invalid broker description: {e}")))?;
        let headers = spec.headers.clone();
        let base = spec.base_url.trim_end_matches('/').to_owned();
        Ok(RestBroker { spec, client: None, secrets, headers, base })
    }

    pub fn spec(&self) -> &Spec {
        &self.spec
    }

    fn redact(&self, mut s: String) -> String {
        for sec in &self.secrets {
            s = s.replace(sec, "***");
        }
        s
    }

    fn client(&mut self) -> Result<&Client> {
        if self.client.is_none() {
            let t = Duration::from_secs(self.spec.timeout_secs.unwrap_or(15).clamp(1, 120));
            let c = Client::builder()
                .timeout(t)
                .connect_timeout(Duration::from_secs(5))
                .redirect(reqwest::redirect::Policy::none())
                .user_agent("ezt-broker/0.1")
                .build()
                .map_err(|e| BrokerError::unavailable(format!("http client: {e}")))?;
            self.client = Some(c);
        }
        Ok(self.client.as_ref().unwrap())
    }

    fn url(&self, path: &str, vars: &Vars, query: &BTreeMap<String, String>) -> Result<String> {
        let mut enc: Vars = Vars::new();
        for (k, v) in vars {
            enc.insert(k, encode(v));
        }
        let mut url = render(path, &enc).map_err(BrokerError::rejected)?;
        if url.starts_with('/') {
            url = format!("{}{}", self.base, url);
        } else if !url.starts_with("http") {
            url = format!("{}/{}", self.base, url);
        }
        let mut first = !url.contains('?');
        for (k, v) in query {
            let v = render(v, &enc).map_err(BrokerError::rejected)?;
            url.push(if first { '?' } else { '&' });
            first = false;
            url.push_str(&format!("{}={}", encode(k), v));
        }
        Ok(url)
    }

    /// One HTTP exchange. GETs are retried once on a transport failure or 5xx.
    fn send(&mut self, method: &str, url: &str, body: Option<(BodyFormat, Value)>) -> Result<Value> {
        let m = Method::from_bytes(method.to_uppercase().as_bytes()).map_err(|_| BrokerError::rejected("bad method"))?;
        let attempts = if m == Method::GET { 2 } else { 1 };
        let mut last = BrokerError::unavailable("no attempt made");
        for _ in 0..attempts {
            let headers = self.headers.clone();
            let client = self.client()?.clone();
            let mut rq = client.request(m.clone(), url);
            for (k, v) in &headers {
                rq = rq.header(k, v);
            }
            if let Some((fmt, b)) = &body {
                match fmt {
                    BodyFormat::Json => rq = rq.json(b),
                    BodyFormat::Form => {
                        let pairs: Vec<(String, String)> =
                            b.as_object().map(|o| o.iter().filter_map(|(k, v)| text(v).map(|t| (k.clone(), t))).collect()).unwrap_or_default();
                        rq = rq.form(&pairs);
                    }
                }
            }
            let resp = match rq.send() {
                Ok(r) => r,
                Err(e) => {
                    last = BrokerError::unavailable(self.redact(format!("cannot reach the brokerage: {}", e.without_url())));
                    continue;
                }
            };
            let status = resp.status();
            let raw = resp.text().unwrap_or_default();
            let parsed: Value = if raw.trim().is_empty() { Value::Null } else { serde_json::from_str(&raw).unwrap_or(Value::String(raw.clone())) };
            if status.is_success() {
                return Ok(parsed);
            }
            let detail = self.redact(error_text(&parsed, &raw));
            let err = match status.as_u16() {
                401 => BrokerError::new(ErrorKind::Auth, format!("{status}: {detail}")),
                429 | 500..=599 => BrokerError::unavailable(format!("{status}: {detail}")),
                _ => BrokerError::rejected(detail),
            };
            if err.kind == ErrorKind::Unavailable {
                last = err;
                continue;
            }
            return Err(err);
        }
        Err(last)
    }

    fn call(&mut self, e: &Endpoint, vars: &Vars) -> Result<Value> {
        let url = self.url(&e.path, vars, &e.query)?;
        let body = match &e.body {
            Some(b) => Some((BodyFormat::Json, render_value(b, vars).map_err(BrokerError::rejected)?)),
            None => None,
        };
        self.send(&e.method, &url, body)
    }

    fn state_of(&self, raw: &str) -> (OrderState, String) {
        match self.spec.states.get(&raw.to_lowercase()) {
            Some(s) => (*s, String::new()),
            None => (OrderState::Open, format!("unrecognised status `{raw}`")),
        }
    }

    fn parse_order(&self, v: &Value, fallback: Option<&OrderRequest>) -> Result<Order> {
        let m = &self.spec.order_map;
        let s = |p: &Option<String>| p.as_ref().and_then(|p| pick(v, p)).and_then(text);
        let n = |p: &Option<String>| p.as_ref().and_then(|p| pick(v, p)).and_then(number);
        let id = pick(v, &m.id).and_then(text).ok_or_else(|| BrokerError::unavailable("the brokerage reply has no order id"))?;
        let raw_state = pick(v, &m.state).and_then(text).unwrap_or_default();
        let (state, mut message) = self.state_of(&raw_state);
        if let Some(extra) = s(&m.message) {
            message = extra;
        }
        let side = match s(&m.side).map(|x| x.to_lowercase()) {
            Some(x) if x.contains("sell") || Some(&x) == self.spec.vocab.sell.as_ref().map(|y| y.to_lowercase()).as_ref() => Side::Sell,
            Some(_) => Side::Buy,
            None => fallback.map_or(Side::Buy, |f| f.side),
        };
        Ok(Order {
            id,
            client_id: s(&m.client_id).or_else(|| fallback.map(|f| f.client_id.clone())).unwrap_or_default(),
            symbol: s(&m.symbol).or_else(|| fallback.map(|f| f.symbol.clone())).unwrap_or_default(),
            side,
            qty: n(&m.qty).or_else(|| fallback.map(|f| f.qty)).unwrap_or(0.0),
            kind: fallback.map_or(Kind::Market, |f| f.kind),
            tif: fallback.map_or(Tif::Day, |f| f.tif),
            state,
            filled_qty: n(&m.filled_qty).unwrap_or(0.0),
            avg_price: n(&m.avg_price).unwrap_or(0.0),
            fees: n(&m.fees).unwrap_or(0.0),
            message,
        })
    }
}

fn error_text(parsed: &Value, raw: &str) -> String {
    for p in ["message", "error", "errors.error", "fault.faultstring", "detail", "errors[0]"] {
        if let Some(t) = pick(parsed, p).and_then(text) {
            return t.chars().take(300).collect();
        }
    }
    raw.chars().take(200).collect()
}

impl Broker for RestBroker {
    fn name(&self) -> &str {
        &self.spec.name
    }

    fn environment(&self) -> Environment {
        self.spec.environment
    }

    fn account(&mut self) -> Result<Account> {
        let e = self.spec.account.clone();
        let reply = self.call(&e, &Vars::new())?;
        let v = e.at.as_ref().and_then(|p| pick(&reply, p)).unwrap_or(&reply);
        let cash = field_num(v, &e.map, "cash").ok_or_else(|| BrokerError::unavailable("the account reply has no cash"))?;
        let equity = field_num(v, &e.map, "equity").unwrap_or(cash);
        let buying_power = field_num(v, &e.map, "buying_power").unwrap_or(cash);
        let currency = e.map.get("currency").and_then(|p| pick(v, p)).and_then(text).unwrap_or_else(|| "USD".into());
        Ok(Account { cash, equity, buying_power, currency })
    }

    fn positions(&mut self) -> Result<Vec<Position>> {
        let e = self.spec.positions.clone();
        let reply = self.call(&e, &Vars::new())?;
        let mut out = Vec::new();
        for item in list_at(&reply, &e.at) {
            let Some(symbol) = e.map.get("symbol").and_then(|p| pick(&item, p)).and_then(text) else { continue };
            let qty = field_num(&item, &e.map, "qty").unwrap_or(0.0);
            if qty.abs() < 1e-12 {
                continue;
            }
            out.push(Position {
                symbol,
                qty,
                avg_cost: field_num(&item, &e.map, "avg_cost").unwrap_or(0.0),
                market_value: field_num(&item, &e.map, "market_value").unwrap_or(0.0),
            });
        }
        Ok(out)
    }

    fn quote(&mut self, symbol: &str) -> Result<Quote> {
        let e = self.spec.quote.clone();
        let vars = Vars::from([("symbol", symbol.to_owned())]);
        let reply = self.call(&e, &vars)?;
        let v = e.at.as_ref().and_then(|p| pick(&reply, p)).unwrap_or(&reply);
        let bid = field_num(v, &e.map, "bid");
        let ask = field_num(v, &e.map, "ask");
        let last = field_num(v, &e.map, "last").or_else(|| bid.zip(ask).map(|(b, a)| (b + a) / 2.0)).filter(|x| *x > 0.0);
        let Some(last) = last else {
            return Err(BrokerError::unavailable(format!("no price for `{symbol}` in the quote reply")));
        };
        Ok(Quote { symbol: symbol.into(), bid: bid.filter(|x| *x > 0.0).unwrap_or(last), ask: ask.filter(|x| *x > 0.0).unwrap_or(last), last })
    }

    fn place_order(&mut self, req: &OrderRequest) -> Result<Order> {
        if !(req.qty.is_finite() && req.qty > 0.0) {
            return Err(BrokerError::rejected(format!("invalid quantity {}", req.qty)));
        }
        let p = self.spec.place_order.clone();
        let voc = &self.spec.vocab;
        let side = match req.side {
            Side::Buy => voc.buy.clone().unwrap_or_else(|| "buy".into()),
            Side::Sell => voc.sell.clone().unwrap_or_else(|| "sell".into()),
        };
        let tif = match req.tif {
            Tif::Day => voc.day.clone().unwrap_or_else(|| "day".into()),
            Tif::Gtc => voc.gtc.clone().unwrap_or_else(|| "gtc".into()),
        };
        let mut vars = Vars::from([
            ("symbol", req.symbol.clone()),
            ("side", side),
            ("qty", num_text(req.qty)),
            ("tif", tif),
            ("client_id", req.client_id.clone()),
        ]);
        let template = match req.kind {
            Kind::Market => &p.market,
            Kind::Limit { limit } => {
                vars.insert("limit", num_text(limit));
                &p.limit
            }
            Kind::Stop { stop } => {
                vars.insert("stop", num_text(stop));
                &p.stop
            }
        };
        let body = render_value(template, &vars).map_err(BrokerError::rejected)?;
        let url = self.url(&p.path, &vars, &BTreeMap::new())?;
        let sent = self.send(&p.method, &url, Some((p.body_format, body)));
        let reply = match sent {
            Ok(r) => r,
            // Unknown outcome (it may have been accepted) or a refusal that could be "already placed":
            // ask the brokerage whether it knows this client id, and only then give up.
            Err(e) if matches!(e.kind, ErrorKind::Unavailable | ErrorKind::Rejected) => {
                if let Some(lookup) = self.spec.order_by_client_id.clone() {
                    let v = Vars::from([("client_id", req.client_id.clone())]);
                    if let Ok(found) = self.call(&lookup, &v) {
                        let item = lookup.at.as_ref().and_then(|p| pick(&found, p)).unwrap_or(&found);
                        if let Ok(o) = self.parse_order(item, Some(req)) {
                            if o.client_id == req.client_id || o.client_id.is_empty() {
                                return Ok(o);
                            }
                        }
                    }
                }
                if e.kind == ErrorKind::Unavailable {
                    return Err(BrokerError::unavailable(format!("{} (the order may or may not have been placed; client id {})", e.message, req.client_id)));
                }
                return Err(e);
            }
            Err(e) => return Err(e),
        };
        let v = p.at.as_ref().and_then(|a| pick(&reply, a)).unwrap_or(&reply);
        let placed = self.parse_order(v, Some(req))?;
        if p.refetch {
            if let Ok(o) = self.order(&placed.id) {
                return Ok(Order { client_id: req.client_id.clone(), kind: req.kind, tif: req.tif, symbol: if o.symbol.is_empty() { req.symbol.clone() } else { o.symbol }, qty: if o.qty > 0.0 { o.qty } else { req.qty }, ..o });
            }
        }
        Ok(placed)
    }

    fn cancel_order(&mut self, id: &str) -> Result<Order> {
        let e = self.spec.cancel_order.clone();
        self.call(&e, &Vars::from([("id", id.to_owned())]))?;
        self.order(id)
    }

    fn order(&mut self, id: &str) -> Result<Order> {
        let e = self.spec.order.clone();
        let reply = self.call(&e, &Vars::from([("id", id.to_owned())]))?;
        let v = e.at.as_ref().and_then(|p| pick(&reply, p)).unwrap_or(&reply);
        self.parse_order(v, None)
    }

    fn open_orders(&mut self) -> Result<Vec<Order>> {
        let e = self.spec.open_orders.clone();
        let reply = self.call(&e, &Vars::new())?;
        list_at(&reply, &e.at).iter().map(|v| self.parse_order(v, None)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_alternatives_and_numbers() {
        let v = json!({"a": {"b": [{"c": "1.5"}, {"c": 2}]}, "n": null, "x": "7"});
        assert_eq!(pick(&v, "a.b[0].c").and_then(number), Some(1.5));
        assert_eq!(pick(&v, "a.b[1].c").and_then(number), Some(2.0));
        assert_eq!(pick(&v, "n|x").and_then(number), Some(7.0));
        assert!(pick(&v, "a.b[2].c").is_none());
        assert!(pick(&v, "$").is_some());
    }

    #[test]
    fn lists_accept_arrays_single_objects_and_nothing() {
        let at = Some("orders.order".to_owned());
        assert_eq!(list_at(&json!({"orders": {"order": [1, 2]}}), &at).len(), 2);
        assert_eq!(list_at(&json!({"orders": {"order": {"id": 1}}}), &at).len(), 1);
        assert_eq!(list_at(&json!({"orders": "null"}), &at).len(), 0);
        assert_eq!(list_at(&json!([1, 2, 3]), &None).len(), 3);
    }

    #[test]
    fn templates_fill_numbers_and_strings_and_complain_about_unknowns() {
        let vars = Vars::from([("qty", "2.5".to_owned()), ("symbol", "AAPL".to_owned())]);
        let v = render_value(&json!({"s": "{symbol}", "q": "{qty:number}", "t": "{qty}"}), &vars).unwrap();
        assert_eq!(v, json!({"s": "AAPL", "q": 2.5, "t": "2.5"}));
        assert!(render_value(&json!("{limit}"), &vars).is_err());
    }

    #[test]
    fn env_placeholders_resolve_and_missing_ones_are_named() {
        let secrets = &mut Vec::new();
        let ok = resolve_env(&json!({"h": "Bearer ${TOK}"}), &|n| (n == "TOK").then(|| "abcd1234".to_owned()), secrets).unwrap();
        assert_eq!(ok, json!({"h": "Bearer abcd1234"}));
        assert_eq!(secrets, &["abcd1234".to_owned()]);
        let err = resolve_env(&json!("${NOPE}"), &|_| None, &mut vec![]).unwrap_err();
        assert!(err.contains("NOPE"));
    }

    #[test]
    fn specs_are_validated() {
        assert!(Spec::parse(&json!({"name": "x"})).is_err());
        assert!(Spec::parse(&json!({"nme": 1})).unwrap_err().contains("unknown field"));
    }
}
