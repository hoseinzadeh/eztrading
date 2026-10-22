//! Brokerage connections: the registry of `brokers/*.json` descriptions, the
//! global kill switch and the live-trading gate.
//!
//! Descriptions are plain files in the workspace (see `docs/brokers.md`). They
//! are read-only for the web UI on purpose: a page that could add a connection
//! could point the server at any URL. Secrets are never in the files; they are
//! `${NAME}` placeholders resolved from the server's environment.

use ezt_broker::*;
use std::result::Result;
use serde::Serialize;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

pub type EnvFn = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

const MAX_SPEC_BYTES: u64 = 64 * 1024;

pub struct Brokers {
    dir: PathBuf,
    env: EnvFn,
    kill: KillSwitch,
    allow_live: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct BrokerInfo {
    pub name: String,
    pub description: String,
    pub environment: Option<Environment>,
    pub host: String,
    pub limits: Option<Limits>,
    /// Environment variables the description needs that are not set.
    pub missing_env: Vec<String>,
    /// Why the description cannot be used, if so.
    pub error: Option<String>,
    pub ready: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct TestStep {
    pub name: String,
    pub ok: bool,
    pub detail: String,
    pub ms: u64,
}

pub fn valid_name(n: &str) -> bool {
    !n.is_empty() && n.len() <= 40 && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

impl Brokers {
    pub fn new(workspace: &Path, allow_live: bool, env: EnvFn) -> Self {
        Self { dir: workspace.join("brokers"), env, kill: KillSwitch::default(), allow_live }
    }

    pub fn from_process_env(workspace: &Path, allow_live: bool) -> Self {
        Self::new(workspace, allow_live, Arc::new(|n| std::env::var(n).ok()))
    }

    pub fn kill_switch(&self) -> KillSwitch {
        self.kill.clone()
    }

    pub fn allow_live(&self) -> bool {
        self.allow_live
    }

    fn read(&self, name: &str) -> Result<Value, String> {
        if !valid_name(name) {
            return Err(format!("`{name}` is not a valid connection name"));
        }
        let path = self.dir.join(format!("{name}.json"));
        let meta = std::fs::metadata(&path).map_err(|_| format!("there is no connection named `{name}` (expected {})", path.display()))?;
        if meta.len() > MAX_SPEC_BYTES {
            return Err(format!("{name}.json is too large"));
        }
        let text = std::fs::read_to_string(&path).map_err(|e| format!("cannot read {name}.json: {e}"))?;
        serde_json::from_str(&text).map_err(|e| format!("{name}.json is not valid JSON: {e}"))
    }

    pub fn spec(&self, name: &str) -> Result<Spec, String> {
        let spec = Spec::parse(&self.read(name)?).map_err(|e| format!("{name}.json: {e}"))?;
        if spec.name != name {
            return Err(format!("{name}.json: `name` is `{}` but must match the file name", spec.name));
        }
        Ok(spec)
    }

    pub fn list(&self) -> Vec<BrokerInfo> {
        let mut out = vec![];
        let Ok(rd) = std::fs::read_dir(&self.dir) else { return out };
        let mut names: Vec<String> = rd
            .flatten()
            .filter_map(|e| e.file_name().to_str().and_then(|n| n.strip_suffix(".json").map(str::to_owned)))
            .filter(|n| valid_name(n))
            .collect();
        names.sort();
        for name in names {
            out.push(match self.spec(&name) {
                Ok(spec) => {
                    let missing: Vec<String> = spec.env_names().into_iter().filter(|n| (self.env)(n).filter(|v| !v.is_empty()).is_none()).collect();
                    let live_blocked = spec.environment == Environment::Live && !self.allow_live;
                    BrokerInfo {
                        host: host_of(&spec.base_url),
                        description: spec.description.clone(),
                        environment: Some(spec.environment),
                        limits: Some(spec.limits.clone()),
                        ready: missing.is_empty() && !live_blocked,
                        error: live_blocked.then(|| "live trading is disabled; start the server with --allow-live".to_owned()),
                        missing_env: missing,
                        name,
                    }
                }
                Err(e) => BrokerInfo { name, description: String::new(), environment: None, host: String::new(), limits: None, missing_env: vec![], error: Some(e), ready: false },
            });
        }
        out
    }

    /// A plain connection (no safety wrapper): only for reads such as the quote feed and the test.
    pub fn connect(&self, name: &str) -> Result<RestBroker, String> {
        let raw = self.read(name)?;
        self.spec(name)?;
        RestBroker::connect(&raw, &*self.env).map_err(|e| e.message)
    }

    /// The limits that apply: the connection's own, tightened by the deployment's.
    pub fn effective_limits(spec: &Spec, asked: &Limits) -> Limits {
        fn min(a: Option<f64>, b: Option<f64>) -> Option<f64> {
            match (a, b) {
                (Some(x), Some(y)) => Some(x.min(y)),
                (x, y) => x.or(y),
            }
        }
        let symbols = match (&spec.limits.symbols, &asked.symbols) {
            (Some(a), Some(b)) => Some(a.iter().filter(|s| b.contains(s)).cloned().collect()),
            (a, b) => a.clone().or_else(|| b.clone()),
        };
        Limits {
            max_order_value: min(spec.limits.max_order_value, asked.max_order_value),
            max_position_value: min(spec.limits.max_position_value, asked.max_position_value),
            max_orders: match (spec.limits.max_orders, asked.max_orders) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            },
            symbols,
        }
    }

    /// Checks everything that has to be true before a deployment may trade through `name`.
    pub fn check_for_trading(&self, name: &str, asked: &Limits, confirm: Option<&str>) -> Result<(Spec, Limits), String> {
        let spec = self.spec(name)?;
        if spec.environment == Environment::Live {
            if !self.allow_live {
                return Err("live trading is disabled: start the server with --allow-live to trade real money".into());
            }
            if confirm != Some(name) {
                return Err(format!("`{name}` trades real money: type its name (`{name}`) to confirm"));
            }
        }
        let limits = Self::effective_limits(&spec, asked);
        if spec.environment == Environment::Live {
            limits.complete_for_live()?;
        }
        Ok((spec, limits))
    }

    /// The broker a deployment trades through: guarded, recorded and able to replay its own history.
    pub fn trading(&self, name: &str, limits: Limits, sink: Sink, replay: Vec<Call>) -> Result<Box<dyn Broker>, String> {
        let raw = self.read(name)?;
        let spec = self.spec(name)?;
        if spec.environment == Environment::Live && !self.allow_live {
            return Err("live trading is disabled: start the server with --allow-live".into());
        }
        let rest = RestBroker::connect(&raw, &*self.env).map_err(|e| e.message)?;
        let guarded = Guarded::new(rest, limits, self.kill.clone())?;
        Ok(Box::new(Replay::new(Recording::new(guarded, sink), replay)))
    }

    /// Read-only checks of a connection. Never places an order.
    pub fn test(&self, name: &str, symbol: Option<&str>) -> Vec<TestStep> {
        let mut steps = vec![];
        let mut step = |name: &str, f: &mut dyn FnMut() -> Result<String, String>| {
            let t = Instant::now();
            let r = f();
            let ms = t.elapsed().as_millis() as u64;
            let (ok, detail) = match r {
                Ok(d) => (true, d),
                Err(e) => (false, e),
            };
            steps.push(TestStep { name: name.to_owned(), ok, detail, ms });
            ok
        };
        let mut conn = None;
        if !step("description and credentials", &mut || {
            let b = self.connect(name)?;
            let s = b.spec();
            let env = serde_json::to_value(s.environment).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default();
            let d = format!("{} ({env}) at {}", s.name, host_of(&s.base_url));
            conn = Some(b);
            Ok(d)
        }) {
            return steps;
        }
        let Some(mut b) = conn else { return steps };
        step("account", &mut || b.account().map(|a| format!("cash {:.2}, equity {:.2}, buying power {:.2} {}", a.cash, a.equity, a.buying_power, a.currency)).map_err(|e| e.message));
        step("positions", &mut || b.positions().map(|p| format!("{} open position(s)", p.len())).map_err(|e| e.message));
        step("open orders", &mut || b.open_orders().map(|o| format!("{} working order(s)", o.len())).map_err(|e| e.message));
        if let Some(sym) = symbol.filter(|s| !s.is_empty()) {
            step(&format!("quote {sym}"), &mut || b.quote(sym).map(|q| format!("bid {} ask {} last {}", q.bid, q.ask, q.last)).map_err(|e| e.message));
        }
        steps
    }
}

fn host_of(url: &str) -> String {
    url.split("://").nth(1).unwrap_or(url).split('/').next().unwrap_or("").to_owned()
}

/// Stands in for a connection that could not be re-established after a restart, so the
/// deployment can still be rebuilt from its log and shown (paused) with the reason.
pub struct Disconnected {
    pub name: String,
    pub environment: Environment,
    pub why: String,
}

impl Disconnected {
    fn fail<T>(&self) -> ezt_broker::Result<T> {
        Err(BrokerError::unavailable(format!("not connected: {}", self.why)))
    }
}

impl Broker for Disconnected {
    fn name(&self) -> &str {
        &self.name
    }
    fn environment(&self) -> Environment {
        self.environment
    }
    fn account(&mut self) -> ezt_broker::Result<Account> {
        self.fail()
    }
    fn positions(&mut self) -> ezt_broker::Result<Vec<Position>> {
        self.fail()
    }
    fn quote(&mut self, _: &str) -> ezt_broker::Result<Quote> {
        self.fail()
    }
    fn place_order(&mut self, _: &OrderRequest) -> ezt_broker::Result<Order> {
        self.fail()
    }
    fn cancel_order(&mut self, _: &str) -> ezt_broker::Result<Order> {
        self.fail()
    }
    fn order(&mut self, _: &str) -> ezt_broker::Result<Order> {
        self.fail()
    }
    fn open_orders(&mut self) -> ezt_broker::Result<Vec<Order>> {
        self.fail()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use ezt_broker::fake::{FakeServer, Flavor};

    pub(crate) fn workspace_with(spec_file: &str, base: &str) -> (tempfile::TempDir, Brokers) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("brokers")).unwrap();
        let text = std::fs::read_to_string(format!("{}/../../brokers/{spec_file}", env!("CARGO_MANIFEST_DIR"))).unwrap();
        let mut v: Value = serde_json::from_str(&text).unwrap();
        v["base_url"] = base.into();
        if let Some(q) = v["quote"]["path"].as_str() {
            if let Some(rest) = q.strip_prefix("https://data.alpaca.markets") {
                v["quote"]["path"] = rest.into();
            }
        }
        std::fs::write(dir.path().join("brokers").join(spec_file), v.to_string()).unwrap();
        let env: EnvFn = Arc::new(|n| match n {
            "ALPACA_KEY_ID" => Some("key-123".into()),
            "ALPACA_SECRET_KEY" => Some("secret-456".into()),
            _ => None,
        });
        let b = Brokers::new(dir.path(), false, env);
        (dir, b)
    }

    #[test]
    fn the_registry_lists_connections_with_their_readiness() {
        let fake = FakeServer::start(Flavor::Alpaca, "key-123", "", 1000.0);
        let (dir, b) = workspace_with("alpaca-paper.json", &fake.url());
        let list = b.list();
        assert_eq!(list.len(), 1);
        assert!(list[0].ready && list[0].missing_env.is_empty(), "{:?}", list[0]);
        assert_eq!(list[0].environment, Some(Environment::Paper));
        std::fs::copy(format!("{}/../../brokers/tradier-sandbox.json", env!("CARGO_MANIFEST_DIR")), dir.path().join("brokers/tradier-sandbox.json")).unwrap();
        std::fs::write(dir.path().join("brokers/broken.json"), "{ nope").unwrap();
        let list = b.list();
        let t = list.iter().find(|i| i.name == "tradier-sandbox").unwrap();
        assert!(!t.ready);
        assert_eq!(t.missing_env, ["TRADIER_ACCOUNT", "TRADIER_TOKEN"]);
        assert!(list.iter().find(|i| i.name == "broken").unwrap().error.as_ref().unwrap().contains("not valid JSON"));
    }

    #[test]
    fn names_cannot_escape_the_brokers_folder() {
        let (_d, b) = workspace_with("alpaca-paper.json", "http://127.0.0.1:1");
        assert!(b.spec("../secret").is_err());
        assert!(b.spec("a/b").is_err());
        assert!(b.spec("nope").is_err());
    }

    #[test]
    fn the_connection_test_reads_but_never_trades() {
        let fake = FakeServer::start(Flavor::Alpaca, "key-123", "", 5000.0);
        fake.price("ACME", 12.5);
        let (_d, b) = workspace_with("alpaca-paper.json", &fake.url());
        let steps = b.test("alpaca-paper", Some("ACME"));
        assert!(steps.iter().all(|s| s.ok), "{steps:?}");
        assert!(steps.iter().any(|s| s.detail.contains("cash 5000")));
        assert!(fake.requests().iter().all(|r| r.starts_with("GET")), "{:?}", fake.requests());
        let bad = FakeServer::start(Flavor::Alpaca, "different", "", 1.0);
        let (_d2, b2) = workspace_with("alpaca-paper.json", &bad.url());
        let steps = b2.test("alpaca-paper", None);
        assert!(!steps.iter().find(|s| s.name == "account").unwrap().ok);
    }

    #[test]
    fn limits_combine_to_the_stricter_value_and_live_needs_confirmation_and_limits() {
        let fake = FakeServer::start(Flavor::Alpaca, "key-123", "", 1000.0);
        let (dir, b) = workspace_with("alpaca-paper.json", &fake.url());
        let asked = Limits { max_order_value: Some(100.0), max_position_value: Some(10_000.0), ..Default::default() };
        let (_, l) = b.check_for_trading("alpaca-paper", &asked, None).unwrap();
        assert_eq!((l.max_order_value, l.max_position_value, l.max_orders), (Some(100.0), Some(5000.0), Some(200)));

        // a live connection
        let mut v: Value = serde_json::from_str(&std::fs::read_to_string(dir.path().join("brokers/alpaca-paper.json")).unwrap()).unwrap();
        v["name"] = "real".into();
        v["environment"] = "live".into();
        v["base_url"] = "https://example.com".into();
        v["limits"] = serde_json::json!({});
        std::fs::write(dir.path().join("brokers/real.json"), v.to_string()).unwrap();
        let e = b.check_for_trading("real", &Limits::default(), Some("real")).unwrap_err();
        assert!(e.contains("--allow-live"), "{e}");
        let env: EnvFn = Arc::new(|_| Some("value1234".into()));
        let b = Brokers::new(dir.path(), true, env);
        assert!(b.check_for_trading("real", &Limits::default(), None).unwrap_err().contains("type its name"));
        assert!(b.check_for_trading("real", &Limits::default(), Some("real")).unwrap_err().contains("max_order_value"));
        let lim = Limits { max_order_value: Some(10.0), max_position_value: Some(50.0), ..Default::default() };
        assert!(b.check_for_trading("real", &lim, Some("real")).is_ok());
    }
}
