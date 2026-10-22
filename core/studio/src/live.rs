//! Live (paper) deployments: run a strategy against a feed on the server,
//! keep running when the browser is closed, and keep an audit trail.
//!
//! Persistence is event sourcing. A deployment is stored as
//!
//! * `deployment.json`: its configuration and current status;
//! * `log.jsonl`: append-only, in order: every bar delivered and every manual
//!   command (flatten, limit change) that influenced the engine;
//! * `events.jsonl`: the audit trail shown as the activity feed.
//!
//! After a restart the engine is rebuilt by replaying `log.jsonl` (the engine is
//! deterministic), so positions, cash and process states come back exactly, and a
//! running deployment simply continues.
//!
//! A deployment trades either on a simulated account (paper trading on the
//! preview simulator: orders fill at the latest price against simulated cash) or
//! through a brokerage connection (`brokers/<name>.json`). For a brokerage the
//! calls it answered are recorded in `log.jsonl` too, so the same replay restores
//! the strategy without sending anything again.

use crate::analyze::analyze;
use crate::brokers::{Brokers, Disconnected};
use crate::feed::{self, Feed, FeedSpec, Poll};
use crate::market::Bar;
use crate::model::{self, Severity};
use crate::sim::{Config, Engine, PositionInfo, ProcessInfo, Risk, SimError, Stats, StepReport, Timeline, Trade};
use ezt_broker as bk;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Duration;

pub const MAX_DEPLOYMENTS: usize = 50;
pub const MAX_RUNNING: usize = 10;
pub const MAX_LIVE_BARS: usize = 100_000;
const MAX_EVENTS_IN_MEMORY: usize = 20_000;
/// Consecutive feed failures after which a deployment pauses itself.
const MAX_FEED_FAILURES: u32 = 10;
const DEFAULT_TAIL: usize = 3000;
#[cfg(not(test))]
const INITIAL_EVENTS: usize = 300;
/// Small in tests so paging can be exercised quickly.
#[cfg(test)]
const INITIAL_EVENTS: usize = 25;

#[derive(Debug)]
pub enum LiveError {
    Invalid(String),
    NotFound,
    Conflict(String),
    Internal(String),
}

impl std::fmt::Display for LiveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LiveError::Invalid(m) | LiveError::Conflict(m) | LiveError::Internal(m) => f.write_str(m),
            LiveError::NotFound => f.write_str("no such deployment"),
        }
    }
}

impl From<SimError> for LiveError {
    fn from(e: SimError) -> Self {
        LiveError::Invalid(e.to_string())
    }
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Running,
    Paused,
    /// Stopped by the user; final.
    Stopped,
    /// The feed ended (a finished replay); final.
    Finished,
    /// The strategy failed at run time; final.
    Error,
}

impl Status {
    pub fn is_final(self) -> bool {
        matches!(self, Status::Stopped | Status::Finished | Status::Error)
    }
}

/// Where a deployment's orders go.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Execution {
    /// A simulated account inside the server.
    #[default]
    Simulated,
    /// A brokerage connection from `brokers/`.
    Broker {
        broker: String,
        /// Tightens the connection's own limits (never loosens them).
        #[serde(default)]
        limits: bk::Limits,
        /// For a connection that trades real money: its name, typed by the user.
        #[serde(default, skip_serializing)]
        confirm: Option<String>,
    },
}

impl Execution {
    pub fn describe(&self) -> String {
        match self {
            Execution::Simulated => "simulated".into(),
            Execution::Broker { broker, .. } => broker.clone(),
        }
    }
}

/// What the user asks for when deploying.
#[derive(Debug, Clone, Deserialize)]
pub struct DeployRequest {
    pub name: String,
    pub source: String,
    #[serde(default)]
    pub strategy_file: Option<String>,
    #[serde(default)]
    pub machine: Option<String>,
    #[serde(default)]
    pub params: BTreeMap<String, serde_json::Value>,
    pub feed: FeedSpec,
    /// Market time per bar in seconds (default: a day for synthetic feeds, the poll interval for live quotes).
    #[serde(default)]
    pub bar_seconds: Option<f64>,
    #[serde(default = "default_cash")]
    pub initial_cash: f64,
    #[serde(default)]
    pub fee_bps: f64,
    #[serde(default)]
    pub max_position_value: Option<f64>,
    /// Flatten and pause when the drawdown from the peak reaches this percentage.
    #[serde(default)]
    pub max_drawdown_pct: Option<f64>,
    /// Start immediately (default). Otherwise the deployment waits, paused.
    #[serde(default = "yes")]
    pub start: bool,
    #[serde(default)]
    pub execution: Execution,
}

fn default_cash() -> f64 {
    100_000.0
}

fn yes() -> bool {
    true
}

/// What is stored in `deployment.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Stored {
    id: String,
    name: String,
    source: String,
    strategy_file: Option<String>,
    machine: Option<String>,
    params: BTreeMap<String, serde_json::Value>,
    feed: FeedSpec,
    bar_seconds: f64,
    initial_cash: f64,
    fee_bps: f64,
    /// The risk guards the engine was created with (later changes are in the log).
    initial_risk: Risk,
    max_drawdown_pct: Option<f64>,
    #[serde(default)]
    execution: Execution,
    created_ms: u64,
    status: Status,
    reason: String,
    started_ms: Option<u64>,
    ended_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
enum Entry {
    Bar { bars: BTreeMap<String, Bar> },
    Flatten,
    Limits { max_position_value: Option<f64> },
    /// Cancel every working order.
    CancelAll,
    /// A call to the brokerage and its answer (replayed after a restart, never re-sent).
    Call(bk::Call),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub seq: u64,
    pub ts_ms: u64,
    pub kind: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

struct Inner {
    stored: Stored,
    engine: Engine,
    dd_breached: bool,
    events: VecDeque<Event>,
    next_seq: u64,
    feed_failures: u32,
    last_feed_error: Option<String>,
    last_bar_ms: Option<u64>,
    log: File,
    evlog: File,
    dir: PathBuf,
    /// Why the brokerage could not be reached after a restart (the deployment is paused).
    connect_error: Option<String>,
}

pub struct Deployment {
    pub id: String,
    inner: Mutex<Inner>,
    kill: bk::KillSwitch,
    /// Asks this deployment's thread to exit without touching its status (server shutdown).
    shutdown: Arc<AtomicBool>,
}

// ---- views -------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct Summary {
    pub id: String,
    pub name: String,
    pub status: Status,
    pub reason: String,
    pub strategy_file: Option<String>,
    pub machine: String,
    pub feed: String,
    /// `simulated`, or the name of the brokerage connection.
    pub execution: String,
    pub environment: Option<bk::Environment>,
    pub symbols: Vec<String>,
    pub created_ms: u64,
    pub started_ms: Option<u64>,
    pub ended_ms: Option<u64>,
    pub last_bar_ms: Option<u64>,
    pub bars: usize,
    pub equity: f64,
    pub initial_cash: f64,
    pub return_pct: f64,
    pub buy_hold_pct: f64,
    pub drawdown_pct: f64,
    pub max_drawdown_pct: f64,
    pub fills: usize,
    pub realized_pnl: f64,
    pub last_price: Option<f64>,
    pub last_label: Option<String>,
    pub processes: Vec<ProcessBrief>,
    /// The last equity points (downsampled) for a sparkline.
    pub spark: Vec<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProcessBrief {
    pub name: String,
    pub state: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Limits {
    pub max_drawdown_pct: Option<f64>,
    pub max_position_value: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BrokerView {
    pub name: String,
    pub environment: Option<bk::Environment>,
    pub account: Option<bk::Account>,
    pub working_orders: usize,
    pub kill_switch: bool,
    pub limits: bk::Limits,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FeedHealth {
    pub consecutive_failures: u32,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Series {
    /// Index of the first bar in this slice (for incremental updates).
    pub from: usize,
    pub labels: Vec<String>,
    pub close: Vec<f64>,
    pub equity: Vec<f64>,
    pub drawdown_pct: Vec<f64>,
    pub position: Vec<f64>,
    pub timelines: Vec<Timeline>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Detail {
    pub summary: Summary,
    pub stats: Stats,
    pub cash: f64,
    pub positions: Vec<PositionInfo>,
    pub processes: Vec<ProcessInfo>,
    pub limits: Limits,
    pub broker: Option<BrokerView>,
    pub orders: Vec<bk::Order>,
    pub feed_health: FeedHealth,
    pub first_close: Option<f64>,
    pub series: Series,
    pub trades_from: usize,
    pub trades: Vec<Trade>,
    pub trade_count: usize,
    pub events: Vec<Event>,
    pub last_seq: u64,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Cursor {
    pub bars_from: Option<usize>,
    pub tail: Option<usize>,
    pub trades_from: Option<usize>,
    pub after_seq: Option<u64>,
}

// ---- the manager -------------------------------------------------------------

pub struct Manager {
    root: PathBuf,
    /// Held while the manager is alive so two servers cannot run the same deployments.
    lock: Mutex<Option<File>>,
    deployments: Mutex<BTreeMap<String, Arc<Deployment>>>,
    threads: Mutex<Vec<JoinHandle<()>>>,
    shutdown: Arc<AtomicBool>,
    counter: AtomicU64,
    brokers: Arc<Brokers>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

fn write_atomic(path: &Path, content: &str) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, content)?;
    std::fs::rename(tmp, path)
}

/// Appends every brokerage call to the deployment's log, one line each.
fn call_sink(dir: &Path) -> Result<bk::Sink, String> {
    let mut file = OpenOptions::new().create(true).append(true).open(dir.join("log.jsonl")).map_err(|e| format!("cannot open log.jsonl: {e}"))?;
    Ok(Arc::new(Mutex::new(move |c: &bk::Call| {
        if let Ok(mut line) = serde_json::to_string(&Entry::Call(c.clone())) {
            line.push('\n');
            let _ = file.write_all(line.as_bytes());
        }
    })))
}

impl Manager {
    /// Opens (creating if needed) the store under `<root>/.live`, restores every
    /// deployment and resumes the ones that were running.
    pub fn open(root: &Path) -> Result<Arc<Manager>, String> {
        Self::open_with(root, Arc::new(Brokers::from_process_env(root, false)))
    }

    pub fn open_with(root: &Path, brokers: Arc<Brokers>) -> Result<Arc<Manager>, String> {
        let root = root.join(".live");
        std::fs::create_dir_all(&root).map_err(|e| format!("cannot create {}: {e}", root.display()))?;
        let lock_file = OpenOptions::new().create(true).write(true).truncate(false).open(root.join("lock")).map_err(|e| format!("cannot open the lock file: {e}"))?;
        match lock_file.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => {
                return Err(format!("another Studio instance is already running the deployments in {}", root.display()));
            }
            Err(std::fs::TryLockError::Error(e)) => return Err(format!("cannot lock {}: {e}", root.display())),
        }
        let m = Arc::new(Manager {
            lock: Mutex::new(Some(lock_file)),
            root,
            deployments: Mutex::new(BTreeMap::new()),
            threads: Mutex::new(vec![]),
            shutdown: Arc::new(AtomicBool::new(false)),
            counter: AtomicU64::new(0),
            brokers,
        });
        let entries = std::fs::read_dir(&m.root).map_err(|e| e.to_string())?;
        for entry in entries.flatten() {
            if !entry.path().join("deployment.json").is_file() {
                continue;
            }
            match m.restore(&entry.path()) {
                Ok(dep) => {
                    lock(&m.deployments).insert(dep.id.clone(), dep);
                }
                Err(e) => eprintln!("warning: cannot restore deployment {}: {e}", entry.path().display()),
            }
        }
        Ok(m)
    }

    /// Stops every deployment thread without changing any status, and waits for them.
    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
        let handles: Vec<_> = lock(&self.threads).drain(..).collect();
        for h in handles {
            let _ = h.join();
        }
        // Release the workspace so another manager (or a restarted server) can take over.
        lock(&self.lock).take();
    }

    pub fn brokers(&self) -> &Arc<Brokers> {
        &self.brokers
    }

    /// Stops all new orders everywhere, pauses every deployment that trades through a
    /// brokerage and cancels its working orders. Positions are left alone (flatten is a
    /// separate, deliberate action). Returns the ids that were paused.
    pub fn emergency_stop(&self) -> Vec<String> {
        self.brokers.kill_switch().trip();
        let deps: Vec<_> = lock(&self.deployments).values().cloned().collect();
        let mut paused = vec![];
        for d in deps {
            if d.emergency() {
                paused.push(d.id.clone());
            }
        }
        paused
    }

    fn new_id(&self) -> String {
        use std::hash::{BuildHasher, Hash, Hasher};
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        (now_ms(), self.counter.fetch_add(1, Ordering::SeqCst)).hash(&mut h);
        format!("{:08x}", h.finish() as u32)
    }

    pub fn list(&self) -> Vec<Summary> {
        let mut out: Vec<Summary> = lock(&self.deployments).values().map(|d| d.summary()).collect();
        out.sort_by(|a, b| b.created_ms.cmp(&a.created_ms));
        out
    }

    pub fn get(&self, id: &str) -> Result<Arc<Deployment>, LiveError> {
        lock(&self.deployments).get(id).cloned().ok_or(LiveError::NotFound)
    }

    fn running_count(&self) -> usize {
        lock(&self.deployments).values().filter(|d| d.status() == Status::Running).count()
    }

    pub fn deploy(self: &Arc<Self>, req: DeployRequest) -> Result<Summary, LiveError> {
        let name = req.name.trim().to_owned();
        if name.is_empty() || name.chars().count() > 60 {
            return Err(LiveError::Invalid("the deployment needs a name of 1 to 60 characters".into()));
        }
        if req.source.len() > 256 * 1024 {
            return Err(LiveError::Invalid("the strategy is too large".into()));
        }
        if lock(&self.deployments).len() >= MAX_DEPLOYMENTS {
            return Err(LiveError::Conflict(format!("at most {MAX_DEPLOYMENTS} deployments; delete some first")));
        }
        if req.start && self.running_count() >= MAX_RUNNING {
            return Err(LiveError::Conflict(format!("at most {MAX_RUNNING} deployments can run at once")));
        }
        if let Some(dd) = req.max_drawdown_pct {
            if !(dd.is_finite() && dd > 0.0 && dd <= 100.0) {
                return Err(LiveError::Invalid("max_drawdown_pct must be between 0 and 100".into()));
            }
        }
        let strategy = model::parse(&req.source).map_err(|d| LiveError::Invalid(format!("the strategy has a syntax error on line {}: {}", d.line, d.message)))?;
        let errors: Vec<_> = analyze(&strategy).into_iter().filter(|d| d.severity == Severity::Error).collect();
        if let Some(first) = errors.first() {
            return Err(LiveError::Invalid(format!("fix {} error(s) before deploying; first: line {}: {}", errors.len(), first.line, first.message)));
        }
        req.feed.validate().map_err(LiveError::Invalid)?;
        let bar_seconds = req.bar_seconds.unwrap_or(match &req.feed {
            FeedSpec::TwelveData { interval_secs, .. } => *interval_secs as f64,
            FeedSpec::Broker { tick_ms, .. } => (*tick_ms as f64 / 1000.0).max(1.0),
            _ => 86_400.0,
        });
        let feed = feed::open_with(&req.feed, bar_seconds, Some(&self.brokers)).map_err(LiveError::Invalid)?;
        let risk = Risk { max_position_value: req.max_position_value };
        let id = self.new_id();
        let dir = self.root.join(&id);

        let mut initial_cash = req.initial_cash;
        let mut fee_bps = req.fee_bps;
        let mut trading: Option<Box<dyn bk::Broker>> = None;
        let mut warning = None;
        if let Execution::Broker { broker, limits, confirm } = &req.execution {
            let (spec, effective) = self.brokers.check_for_trading(broker, limits, confirm.as_deref()).map_err(LiveError::Invalid)?;
            if spec.environment == bk::Environment::Live && matches!(req.feed, FeedSpec::Replay { .. } | FeedSpec::Simulated { .. }) {
                return Err(LiveError::Invalid("a connection that trades real money needs real prices: use a broker or Twelve Data feed, not a simulated one".into()));
            }
            // Reach the brokerage once, on a separate unrecorded connection, to check the credentials
            // and learn the starting equity (the figure returns are measured against).
            let mut probe = self.brokers.connect(broker).map_err(LiveError::Invalid)?;
            let account = bk::Broker::account(&mut probe).map_err(|e| LiveError::Invalid(format!("cannot read the account at {broker}: {e}")))?;
            if !(account.equity.is_finite() && account.equity > 0.0) {
                return Err(LiveError::Invalid(format!("the account at {broker} has no equity to trade with")));
            }
            let held = bk::Broker::positions(&mut probe).map_err(|e| LiveError::Invalid(format!("cannot read positions at {broker}: {e}")))?;
            if !held.is_empty() {
                warning = Some(format!("the account already holds {} position(s); this strategy only manages what it buys itself", held.len()));
            }
            initial_cash = account.equity;
            fee_bps = 0.0;
            std::fs::create_dir_all(&dir).map_err(|e| LiveError::Internal(format!("cannot create the deployment folder: {e}")))?;
            let sink = call_sink(&dir).map_err(LiveError::Internal)?;
            trading = Some(self.brokers.trading(broker, effective, sink, vec![]).map_err(LiveError::Invalid)?);
        }
        let cfg = Config { machine: req.machine.clone(), params: req.params.clone(), initial_cash, fee_bps, risk: risk.clone(), client_prefix: Some(id.clone()) };
        let strategy = Arc::new(strategy);
        let engine = match trading {
            Some(b) => Engine::with_broker(strategy, &req.feed.symbols(), bar_seconds, &cfg, b),
            None => Engine::new(strategy, &req.feed.symbols(), bar_seconds, &cfg),
        };
        let engine = match engine {
            Ok(e) => e,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&dir);
                return Err(e.into());
            }
        };
        std::fs::create_dir_all(&dir).map_err(|e| LiveError::Internal(format!("cannot create the deployment folder: {e}")))?;
        let execution = match &req.execution {
            Execution::Broker { broker, limits, .. } => Execution::Broker { broker: broker.clone(), limits: limits.clone(), confirm: None },
            Execution::Simulated => Execution::Simulated,
        };
        let stored = Stored {
            id: id.clone(),
            name,
            source: req.source,
            strategy_file: req.strategy_file,
            machine: req.machine,
            params: req.params,
            feed: req.feed,
            bar_seconds,
            initial_cash,
            fee_bps,
            initial_risk: risk,
            max_drawdown_pct: req.max_drawdown_pct,
            execution,
            created_ms: now_ms(),
            status: if req.start { Status::Running } else { Status::Paused },
            reason: if req.start { String::new() } else { "deployed paused; resume it to start".into() },
            started_ms: req.start.then(now_ms),
            ended_ms: None,
        };
        let dep = self.build(stored, engine, &dir, VecDeque::new(), 1).map_err(LiveError::Internal)?;
        {
            let mut g = dep.lock();
            let msg = format!("deployed from {}", g.stored.strategy_file.clone().unwrap_or_else(|| "the editor".into()));
            g.event("deployed", msg, None);
            if let Some(w) = warning {
                g.event("warning", w, None);
            }
            if req.start {
                g.event("started", "started".into(), None);
            }
            g.persist().map_err(LiveError::Internal)?;
        }
        lock(&self.deployments).insert(id.clone(), dep.clone());
        self.spawn(&dep, feed);
        Ok(dep.summary())
    }

    fn build(&self, stored: Stored, engine: Engine, dir: &Path, events: VecDeque<Event>, next_seq: u64) -> Result<Arc<Deployment>, String> {
        let open = |name: &str| OpenOptions::new().create(true).append(true).open(dir.join(name)).map_err(|e| format!("cannot open {name}: {e}"));
        let inner = Inner {
            log: open("log.jsonl")?,
            evlog: open("events.jsonl")?,
            stored: stored.clone(),
            engine,
            dd_breached: false,
            events,
            next_seq,
            feed_failures: 0,
            last_feed_error: None,
            last_bar_ms: None,
            dir: dir.to_path_buf(),
            connect_error: None,
        };
        Ok(Arc::new(Deployment { id: stored.id, inner: Mutex::new(inner), kill: self.brokers.kill_switch(), shutdown: self.shutdown.clone() }))
    }

    fn spawn(&self, dep: &Arc<Deployment>, feed: Box<dyn Feed>) {
        let tick = dep.lock().stored.feed.tick();
        let dep = dep.clone();
        let handle = std::thread::Builder::new().name(format!("deployment-{}", dep.id)).spawn(move || dep.run(feed, tick)).expect("spawn");
        lock(&self.threads).push(handle);
    }

    fn restore(&self, dir: &Path) -> Result<Arc<Deployment>, String> {
        let text = std::fs::read_to_string(dir.join("deployment.json")).map_err(|e| e.to_string())?;
        let mut stored: Stored = serde_json::from_str(&text).map_err(|e| format!("deployment.json: {e}"))?;
        let strategy = model::parse(&stored.source).map_err(|d| format!("stored strategy: {}", d.message))?;
        let cfg = Config {
            machine: stored.machine.clone(),
            params: stored.params.clone(),
            initial_cash: stored.initial_cash,
            fee_bps: stored.fee_bps,
            risk: stored.initial_risk.clone(),
            client_prefix: Some(stored.id.clone()),
        };

        let mut entries: Vec<Entry> = vec![];
        if let Ok(f) = File::open(dir.join("log.jsonl")) {
            for line in BufReader::new(f).lines().map_while(Result::ok) {
                if let Ok(entry) = serde_json::from_str::<Entry>(&line) {
                    entries.push(entry); // an unreadable (torn) line is skipped
                }
            }
        }

        let mut connect_error = None;
        let strategy = Arc::new(strategy);
        let mut engine = match &stored.execution {
            Execution::Simulated => Engine::new(strategy, &stored.feed.symbols(), stored.bar_seconds, &cfg),
            Execution::Broker { broker, limits, .. } => {
                let calls: Vec<bk::Call> = entries.iter().filter_map(|e| if let Entry::Call(c) = e { Some(c.clone()) } else { None }).collect();
                let sink = call_sink(dir)?;
                let live = match self.brokers.spec(broker) {
                    Ok(spec) => spec.environment == bk::Environment::Live,
                    Err(_) => false,
                };
                let effective = self.brokers.spec(broker).map(|spec| Brokers::effective_limits(&spec, limits));
                let made = effective.and_then(|l| self.brokers.trading(broker, l, sink.clone(), calls.clone()));
                let b: Box<dyn bk::Broker> = match made {
                    Ok(b) => b,
                    Err(why) => {
                        connect_error = Some(why.clone());
                        let env = if live { bk::Environment::Live } else { bk::Environment::Paper };
                        Box::new(bk::Replay::new(Disconnected { name: broker.clone(), environment: env, why }, calls))
                    }
                };
                Engine::with_broker(strategy, &stored.feed.symbols(), stored.bar_seconds, &cfg, b)
            }
        }
        .map_err(|e| e.to_string())?;

        // Replay the log: this restores positions, cash and every process state.
        // Brokerage calls are answered from the recorded ones, nothing is sent again.
        let mut replay_error = None;
        for entry in entries {
            let r = match entry {
                Entry::Bar { bars } => engine.step(&bars).map(|_| ()),
                Entry::Flatten => engine.flatten().map(|_| ()),
                Entry::Limits { max_position_value } => engine.set_max_position_value(max_position_value),
                Entry::CancelAll => engine.cancel_all_orders().map(|_| ()),
                Entry::Call(_) => continue,
            };
            if let Err(e) = r {
                replay_error = Some(e.to_string());
                break;
            }
        }
        let mut events: VecDeque<Event> = VecDeque::new();
        let mut next_seq = 1;
        if let Ok(f) = File::open(dir.join("events.jsonl")) {
            for line in BufReader::new(f).lines().map_while(Result::ok) {
                if let Ok(ev) = serde_json::from_str::<Event>(&line) {
                    next_seq = next_seq.max(ev.seq + 1);
                    events.push_back(ev);
                    if events.len() > MAX_EVENTS_IN_MEMORY {
                        events.pop_front();
                    }
                }
            }
        }
        if let Some(e) = replay_error {
            stored.status = Status::Error;
            stored.reason = format!("replaying the log failed: {e}");
        }
        let was_running = stored.status == Status::Running;
        if !stored.status.is_final() {
            if let Some(why) = &connect_error {
                stored.status = Status::Paused;
                stored.reason = format!("cannot reach the brokerage after a restart: {why}");
            } else if was_running && engine.broker_environment() == bk::Environment::Live {
                stored.status = Status::Paused;
                stored.reason = "deployments that trade real money are not resumed automatically after a restart; check the account, then resume".into();
            }
        }
        let feed = if stored.status.is_final() {
            None
        } else {
            match feed::open_with(&stored.feed, stored.bar_seconds, Some(&self.brokers)) {
                Ok(mut f) => {
                    f.skip(engine.bar_count());
                    Some(f)
                }
                Err(e) => {
                    stored.status = Status::Paused;
                    stored.reason = format!("could not reopen the feed after a restart: {e}");
                    None
                }
            }
        };
        let dep = self.build(stored, engine, dir, events, next_seq)?;
        {
            let mut g = dep.lock();
            g.connect_error = connect_error;
            let bars = g.engine.bar_count();
            if was_running && g.stored.status == Status::Running {
                g.event("restored", format!("the server restarted; resumed at bar {bars}"), None);
            } else if g.stored.status == Status::Paused && was_running {
                let reason = g.stored.reason.clone();
                g.event("restored", format!("the server restarted; paused: {reason}"), None);
            }
            g.persist()?;
        }
        if let Some(feed) = feed {
            self.spawn(&dep, feed);
        }
        Ok(dep)
    }

    pub fn delete(&self, id: &str) -> Result<(), LiveError> {
        let dep = self.get(id)?;
        if !dep.status().is_final() {
            return Err(LiveError::Conflict("stop the deployment before deleting it".into()));
        }
        lock(&self.deployments).remove(id);
        drop(dep);
        std::fs::remove_dir_all(self.root.join(id)).map_err(|e| LiveError::Internal(format!("cannot delete the folder: {e}")))
    }

    pub fn resume(&self, id: &str) -> Result<Summary, LiveError> {
        let dep = self.get(id)?;
        if dep.status() != Status::Running && self.running_count() >= MAX_RUNNING {
            return Err(LiveError::Conflict(format!("at most {MAX_RUNNING} deployments can run at once")));
        }
        dep.resume()?;
        Ok(dep.summary())
    }
}

// ---- one deployment ----------------------------------------------------------

impl Inner {
    fn event(&mut self, kind: &str, message: String, data: Option<serde_json::Value>) {
        let ev = Event { seq: self.next_seq, ts_ms: now_ms(), kind: kind.to_owned(), message, data };
        self.next_seq += 1;
        if let Ok(line) = serde_json::to_string(&ev) {
            let _ = writeln!(self.evlog, "{line}");
        }
        self.events.push_back(ev);
        if self.events.len() > MAX_EVENTS_IN_MEMORY {
            self.events.pop_front();
        }
    }

    fn persist(&self) -> Result<(), String> {
        let json = serde_json::to_string_pretty(&self.stored).map_err(|e| e.to_string())?;
        write_atomic(&self.dir.join("deployment.json"), &json).map_err(|e| format!("cannot save the deployment: {e}"))
    }

    fn append(&mut self, entry: &Entry) -> Result<(), String> {
        let line = serde_json::to_string(entry).map_err(|e| e.to_string())?;
        writeln!(self.log, "{line}").and_then(|_| self.log.flush()).map_err(|e| format!("cannot write the log: {e}"))
    }

    fn set_status(&mut self, status: Status, reason: &str) {
        self.stored.status = status;
        self.stored.reason = reason.to_owned();
        if status.is_final() {
            self.stored.ended_ms = Some(now_ms());
        }
        let _ = self.persist();
    }

    fn trade_events(&mut self, trades: &[Trade]) {
        for t in trades {
            let pnl = t.pnl.map(|p| format!(" (P&L {p:+.2})")).unwrap_or_default();
            let msg = format!("{} {:.4} {} @ {:.4}{pnl}", t.side.to_uppercase(), t.shares, t.symbol, t.price);
            let data = serde_json::to_value(t).ok();
            self.event("fill", msg, data);
        }
    }

    fn after_step(&mut self, report: &StepReport) {
        self.last_bar_ms = Some(now_ms());
        self.feed_failures = 0;
        self.trade_events(&report.trades);
        for t in &report.transitions {
            self.event("state", format!("{}: {} → {}", t.process, t.from, t.to), serde_json::to_value(t).ok());
        }
        for l in &report.logs {
            self.event("log", l.clone(), None);
        }
        for o in &report.orders {
            self.event("order", o.clone(), None);
        }
        for n in &report.notes {
            let kind = if n.starts_with("risk limit") || n.starts_with("blocked") { "risk" } else { "warning" };
            self.event(kind, n.clone(), None);
        }
        if let Some(limit) = self.stored.max_drawdown_pct {
            if report.drawdown_pct >= limit && !self.dd_breached {
                self.dd_breached = true;
                let msg = format!("max drawdown {limit}% reached ({:.2}%): closing all positions and pausing", report.drawdown_pct);
                self.event("risk", msg.clone(), None);
                match self.flatten_inner() {
                    Ok(()) => self.set_status(Status::Paused, &format!("paused by the drawdown limit ({limit}%)")),
                    Err(e) => self.set_status(Status::Error, &e),
                }
            } else if report.drawdown_pct < limit {
                self.dd_breached = false;
            }
        }
    }

    fn broker_env(&self) -> Option<bk::Environment> {
        matches!(self.stored.execution, Execution::Broker { .. }).then(|| self.engine.broker_environment())
    }

    fn cancel_inner(&mut self) -> Result<usize, String> {
        self.append(&Entry::CancelAll)?;
        let (n, notes) = self.engine.cancel_all_orders().map_err(|e| e.to_string())?;
        for note in notes {
            self.event("warning", note, None);
        }
        Ok(n)
    }

    fn flatten_inner(&mut self) -> Result<(), String> {
        self.append(&Entry::Flatten)?;
        let trades = self.engine.flatten().map_err(|e| e.to_string())?;
        if trades.is_empty() {
            self.event("flatten", "flatten requested; there was nothing to sell".into(), None);
        } else {
            self.event("flatten", format!("closed {} position(s) at the latest price", trades.len()), None);
            self.trade_events(&trades);
        }
        Ok(())
    }

    fn summary(&self) -> Summary {
        let stats = self.engine.stats();
        let primary = self.engine.primary();
        let bars = self.engine.bars(primary);
        let rec = self.engine.recording();
        let n = rec.equity.len();
        let step = (n / 80).max(1);
        let mut spark: Vec<f64> = rec.equity.iter().step_by(step).copied().collect();
        if let (Some(last), true) = (rec.equity.last(), n > 0) {
            if spark.last() != Some(last) {
                spark.push(*last);
            }
        }
        Summary {
            id: self.stored.id.clone(),
            name: self.stored.name.clone(),
            status: self.stored.status,
            reason: self.stored.reason.clone(),
            strategy_file: self.stored.strategy_file.clone(),
            machine: self.engine.machine_name().to_owned(),
            feed: self.stored.feed.describe(),
            execution: self.stored.execution.describe(),
            environment: self.broker_env(),
            symbols: self.stored.feed.symbols(),
            created_ms: self.stored.created_ms,
            started_ms: self.stored.started_ms,
            ended_ms: self.stored.ended_ms,
            last_bar_ms: self.last_bar_ms,
            bars: n,
            equity: stats.final_equity,
            initial_cash: stats.initial_cash,
            return_pct: stats.total_return_pct,
            buy_hold_pct: stats.buy_hold_return_pct,
            drawdown_pct: -rec.drawdown_pct.last().copied().unwrap_or(0.0),
            max_drawdown_pct: stats.max_drawdown_pct,
            fills: stats.fills,
            realized_pnl: stats.realized_pnl,
            last_price: bars.last().map(|b| b.close),
            last_label: bars.last().map(|b| b.label.clone()),
            processes: self.engine.processes().into_iter().map(|p| ProcessBrief { name: p.name, state: p.state }).collect(),
            spark,
        }
    }
}

impl Deployment {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        lock(&self.inner)
    }

    pub fn status(&self) -> Status {
        self.lock().stored.status
    }

    pub fn summary(&self) -> Summary {
        self.lock().summary()
    }

    pub fn source(&self) -> (String, Option<String>, Option<String>) {
        let g = self.lock();
        (g.stored.source.clone(), g.stored.strategy_file.clone(), g.stored.machine.clone())
    }

    pub fn detail(&self, cur: &Cursor) -> Detail {
        let g = self.lock();
        let summary = g.summary();
        let primary = g.engine.primary().to_owned();
        let bars = g.engine.bars(&primary);
        let rec = g.engine.recording();
        let n = bars.len();
        let from = match cur.bars_from {
            Some(f) => f.min(n),
            None => n.saturating_sub(cur.tail.unwrap_or(DEFAULT_TAIL)),
        };
        let timelines = g.engine.timelines_from(from);
        let trades = g.engine.trades();
        let trades_from = cur.trades_from.unwrap_or(0).min(trades.len());
        // A first request (no cursor) gets the latest events only; later ones page forward.
        let events: Vec<Event> = match cur.after_seq {
            Some(after) => g.events.iter().filter(|e| e.seq > after).cloned().collect(),
            None => g.events.iter().skip(g.events.len().saturating_sub(INITIAL_EVENTS)).cloned().collect(),
        };
        Detail {
            stats: g.engine.stats(),
            cash: g.engine.cash(),
            positions: g.engine.positions(),
            processes: g.engine.processes(),
            limits: Limits { max_drawdown_pct: g.stored.max_drawdown_pct, max_position_value: g.engine.max_position_value() },
            broker: match &g.stored.execution {
                Execution::Simulated => None,
                Execution::Broker { broker, limits, .. } => Some(BrokerView {
                    name: broker.clone(),
                    environment: g.broker_env(),
                    account: g.engine.account().cloned(),
                    working_orders: g.engine.working_orders(),
                    kill_switch: self.kill.is_tripped(),
                    limits: limits.clone(),
                    error: g.connect_error.clone(),
                }),
            },
            orders: g.engine.recent_orders(50),
            feed_health: FeedHealth { consecutive_failures: g.feed_failures, last_error: g.last_feed_error.clone() },
            first_close: bars.first().map(|b| b.close),
            series: Series {
                from,
                labels: bars[from..].iter().map(|b| b.label.clone()).collect(),
                close: bars[from..].iter().map(|b| b.close).collect(),
                equity: rec.equity[from..].to_vec(),
                drawdown_pct: rec.drawdown_pct[from..].to_vec(),
                position: rec.position[from..].to_vec(),
                timelines,
            },
            trades_from,
            trades: trades[trades_from..].to_vec(),
            trade_count: trades.len(),
            events,
            last_seq: g.next_seq - 1,
            warnings: g.engine.warnings().to_vec(),
            summary,
        }
    }

    // ---- commands -------------------------------------------------------------

    pub fn pause(&self) -> Result<(), LiveError> {
        let mut g = self.lock();
        match g.stored.status {
            Status::Running => {
                g.set_status(Status::Paused, "paused by the user");
                g.event("paused", "paused by the user (the bar in flight, if any, is still processed)".into(), None);
                Ok(())
            }
            Status::Paused => Ok(()),
            _ => Err(LiveError::Conflict("a finished deployment cannot be paused".into())),
        }
    }

    /// The emergency stop for this deployment; returns whether it was affected.
    fn emergency(&self) -> bool {
        let mut g = self.lock();
        if g.stored.status.is_final() || !matches!(g.stored.execution, Execution::Broker { .. }) {
            return false;
        }
        let cancelled = g.cancel_inner();
        if g.stored.status == Status::Running {
            g.set_status(Status::Paused, "paused by the emergency stop");
        }
        let detail = match cancelled {
            Ok(n) => format!("emergency stop: paused, {n} working order(s) cancelled, positions left open"),
            Err(e) => format!("emergency stop: paused; cancelling orders failed: {e}"),
        };
        g.event("risk", detail, None);
        true
    }

    pub fn cancel_orders(&self) -> Result<usize, LiveError> {
        let mut g = self.lock();
        if g.stored.status.is_final() {
            return Err(LiveError::Conflict("a finished deployment cannot trade".into()));
        }
        let n = g.cancel_inner().map_err(LiveError::Internal)?;
        g.event("cancel", format!("cancelled {n} working order(s)"), None);
        Ok(n)
    }

    pub fn resume(&self) -> Result<(), LiveError> {
        let mut g = self.lock();
        if g.stored.status == Status::Paused && matches!(g.stored.execution, Execution::Broker { .. }) {
            if let Some(why) = &g.connect_error {
                return Err(LiveError::Conflict(format!("cannot resume: the brokerage is not reachable ({why}); fix it and restart the server")));
            }
            if self.kill.is_tripped() {
                return Err(LiveError::Conflict("the kill switch is on; reset it before resuming".into()));
            }
        }
        match g.stored.status {
            Status::Paused => {
                if g.stored.started_ms.is_none() {
                    g.stored.started_ms = Some(now_ms());
                }
                g.feed_failures = 0;
                g.set_status(Status::Running, "");
                g.event("resumed", "resumed".into(), None);
                Ok(())
            }
            Status::Running => Ok(()),
            _ => Err(LiveError::Conflict("a finished deployment cannot be resumed; deploy it again instead".into())),
        }
    }

    pub fn stop(&self, flatten: bool) -> Result<(), LiveError> {
        let mut g = self.lock();
        if g.stored.status.is_final() {
            return Ok(());
        }
        if flatten {
            g.flatten_inner().map_err(LiveError::Internal)?;
        }
        g.set_status(Status::Stopped, "stopped by the user");
        g.event("stopped", if flatten { "stopped; positions were closed first".into() } else { "stopped; positions were left open".into() }, None);
        Ok(())
    }

    pub fn flatten(&self) -> Result<(), LiveError> {
        let mut g = self.lock();
        if g.stored.status.is_final() {
            return Err(LiveError::Conflict("a finished deployment cannot trade".into()));
        }
        g.flatten_inner().map_err(LiveError::Internal)
    }

    pub fn set_limits(&self, max_drawdown_pct: Option<Option<f64>>, max_position_value: Option<Option<f64>>) -> Result<(), LiveError> {
        let mut g = self.lock();
        if g.stored.status.is_final() {
            return Err(LiveError::Conflict("a finished deployment cannot be changed".into()));
        }
        if let Some(Some(dd)) = max_drawdown_pct {
            if !(dd.is_finite() && dd > 0.0 && dd <= 100.0) {
                return Err(LiveError::Invalid("max_drawdown_pct must be between 0 and 100".into()));
            }
        }
        if let Some(Some(c)) = max_position_value {
            if !(c.is_finite() && c > 0.0) {
                return Err(LiveError::Invalid("max_position_value must be positive".into()));
            }
        }
        if let Some(dd) = max_drawdown_pct {
            g.stored.max_drawdown_pct = dd;
            g.dd_breached = false;
            g.event("limits", format!("max drawdown limit set to {}", dd.map_or("none".into(), |v| format!("{v}%"))), None);
        }
        if let Some(cap) = max_position_value {
            g.append(&Entry::Limits { max_position_value: cap }).map_err(LiveError::Internal)?;
            g.engine.set_max_position_value(cap)?;
            g.event("limits", format!("max position value set to {}", cap.map_or("none".into(), |v| format!("{v}"))), None);
        }
        g.persist().map_err(LiveError::Internal)
    }

    // ---- the thread -------------------------------------------------------------

    fn run(self: Arc<Self>, mut feed: Box<dyn Feed>, tick: Duration) {
        let slice = Duration::from_millis(20);
        loop {
            let mut waited = Duration::ZERO;
            while waited < tick {
                let step = slice.min(tick - waited);
                std::thread::sleep(step);
                waited += step;
                if self.shutdown.load(Ordering::Relaxed) || self.status().is_final() {
                    return;
                }
            }
            if self.status() != Status::Running {
                continue; // paused: keep the thread, skip the tick
            }
            match feed.next() {
                Poll::Bars(bars) => {
                    if !self.apply_bar(bars) {
                        return;
                    }
                }
                Poll::End => {
                    let mut g = self.lock();
                    if !g.stored.status.is_final() {
                        g.set_status(Status::Finished, "the replay reached the end of its data");
                        g.event("finished", "the replay reached the end of its data".into(), None);
                    }
                    return;
                }
                Poll::Failed(e) => self.feed_failed(e),
            }
        }
    }

    /// Applies a delivered bar. Returns false when the deployment is finished.
    fn apply_bar(&self, bars: BTreeMap<String, Bar>) -> bool {
        let mut g = self.lock();
        if g.stored.status.is_final() {
            return false;
        }
        if g.engine.bar_count() >= MAX_LIVE_BARS {
            g.set_status(Status::Finished, "the bar limit was reached");
            g.event("finished", format!("stopped after {MAX_LIVE_BARS} bars (the per-deployment limit)"), None);
            return false;
        }
        if let Err(e) = g.append(&Entry::Bar { bars: bars.clone() }) {
            g.set_status(Status::Error, &e);
            g.event("error", e, None);
            return false;
        }
        match g.engine.step(&bars) {
            Ok(report) => {
                g.after_step(&report);
                !g.stored.status.is_final()
            }
            Err(e) => {
                let msg = e.to_string();
                g.set_status(Status::Error, &msg);
                g.event("error", format!("the strategy failed: {msg}"), None);
                false
            }
        }
    }

    fn feed_failed(&self, error: String) {
        let mut g = self.lock();
        g.feed_failures += 1;
        g.last_feed_error = Some(error.clone());
        let failures = g.feed_failures;
        g.event("feed", format!("feed error ({failures} in a row): {error}"), None);
        if g.feed_failures >= MAX_FEED_FAILURES && g.stored.status == Status::Running {
            let reason = format!("paused after {MAX_FEED_FAILURES} feed failures in a row; last: {error}");
            g.set_status(Status::Paused, &reason);
            g.event("paused", reason, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feed::ReplayData;
    use crate::market::Generator;
    use std::time::Instant;

    const THRESHOLD: &str = include_str!("../../../examples/threshold.sm");

    /// Buys one share on every bar; handy for deterministic position changes.
    const ALWAYS_BUY: &str = "machine M { param sym: Symbol; param account: Account = SandBox; init { launch (p, A); }
        process p { listen Tickers[sym]; state A: goto A { account.buy(sym, 1); } } }";

    fn wait_until(what: &str, mut cond: impl FnMut() -> bool) {
        let start = Instant::now();
        while !cond() {
            assert!(start.elapsed() < Duration::from_secs(15), "timed out waiting for: {what}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn sim_feed(tick_ms: u64) -> FeedSpec {
        FeedSpec::Simulated { symbol: "TEST".into(), generator: Generator { kind: "sine".into(), seed: 3, vol: 0.004, period: 40, amplitude: 0.2, ..Default::default() }, tick_ms }
    }

    fn csv_feed(prices: &[f64], tick_ms: u64) -> FeedSpec {
        let csv = format!("close\n{}", prices.iter().map(|p| p.to_string()).collect::<Vec<_>>().join("\n"));
        FeedSpec::Replay { symbol: "TEST".into(), data: ReplayData::Csv { csv }, tick_ms }
    }

    fn request(source: &str, feed: FeedSpec) -> DeployRequest {
        DeployRequest {
            name: "test deployment".into(),
            source: source.into(),
            strategy_file: Some("t.sm".into()),
            machine: None,
            params: serde_json::from_value(serde_json::json!({"floor": 95, "ceiling": 105})).unwrap(),
            feed,
            bar_seconds: None,
            initial_cash: 100_000.0,
            fee_bps: 0.0,
            max_position_value: None,
            max_drawdown_pct: None,
            start: true,
            execution: Execution::Simulated,
        }
    }

    fn bars(d: &Deployment) -> usize {
        d.summary().bars
    }

    #[test]
    fn a_replay_runs_to_the_end_and_records_everything() {
        let dir = tempfile::tempdir().unwrap();
        let m = Manager::open(dir.path()).unwrap();
        let feed = FeedSpec::Replay { symbol: "TEST".into(), data: ReplayData::Generate { generator: Generator { kind: "sine".into(), bars: 80, seed: 3, vol: 0.004, period: 40, amplitude: 0.2, ..Default::default() } }, tick_ms: 20 };
        let s = m.deploy(request(THRESHOLD, feed)).unwrap();
        assert_eq!(s.status, Status::Running);
        let dep = m.get(&s.id).unwrap();
        wait_until("replay to finish", || dep.status() == Status::Finished);
        let d = dep.detail(&Cursor::default());
        assert_eq!(d.summary.bars, 80);
        assert_eq!(d.series.close.len(), 80);
        assert_eq!(d.series.equity.len(), 80);
        assert!(d.trade_count >= 2, "a sine wave crosses 95 and 105 repeatedly");
        let kinds: Vec<&str> = d.events.iter().map(|e| e.kind.as_str()).collect();
        for k in ["deployed", "started", "fill", "state", "finished"] {
            assert!(kinds.contains(&k), "missing {k} in {kinds:?}");
        }
        assert!(d.events.windows(2).all(|w| w[0].seq + 1 == w[1].seq), "event sequence numbers are consecutive");
        assert_eq!(d.summary.reason, "the replay reached the end of its data");
        m.shutdown();
    }

    #[test]
    fn cursors_return_only_what_is_new() {
        let dir = tempfile::tempdir().unwrap();
        let m = Manager::open(dir.path()).unwrap();
        let s = m.deploy(request(THRESHOLD, sim_feed(20))).unwrap();
        let dep = m.get(&s.id).unwrap();
        wait_until("a few bars", || bars(&dep) >= 5);
        let first = dep.detail(&Cursor::default());
        let n = first.summary.bars;
        wait_until("more bars", || bars(&dep) >= n + 3);
        let next = dep.detail(&Cursor { bars_from: Some(n), trades_from: Some(first.trade_count), after_seq: Some(first.last_seq), tail: None });
        assert_eq!(next.series.from, n);
        assert!(next.series.close.len() >= 3);
        assert!(next.events.iter().all(|e| e.seq > first.last_seq));
        let tail = dep.detail(&Cursor { tail: Some(2), ..Default::default() });
        assert_eq!(tail.series.close.len(), 2);
        assert_eq!(tail.series.timelines[0].series.len(), 2);
        m.shutdown();
    }

    #[test]
    fn a_first_request_gets_only_the_latest_events() {
        let dir = tempfile::tempdir().unwrap();
        let m = Manager::open(dir.path()).unwrap();
        let s = m.deploy(request(ALWAYS_BUY, sim_feed(20))).unwrap();
        let dep = m.get(&s.id).unwrap();
        wait_until("many events", || dep.detail(&Cursor { after_seq: Some(0), ..Default::default() }).events.len() > INITIAL_EVENTS + 20);
        let first = dep.detail(&Cursor::default());
        assert_eq!(first.events.len(), INITIAL_EVENTS);
        assert_eq!(first.events.last().unwrap().seq, first.last_seq.min(first.events.last().unwrap().seq));
        let all = dep.detail(&Cursor { after_seq: Some(0), ..Default::default() });
        assert!(all.events.len() > INITIAL_EVENTS, "paging forward from 0 returns everything");
        m.shutdown();
    }

    #[test]
    fn pause_stops_the_clock_and_resume_restarts_it() {
        let dir = tempfile::tempdir().unwrap();
        let m = Manager::open(dir.path()).unwrap();
        let s = m.deploy(request(THRESHOLD, sim_feed(20))).unwrap();
        let dep = m.get(&s.id).unwrap();
        wait_until("two bars", || bars(&dep) >= 2);
        dep.pause().unwrap();
        std::thread::sleep(Duration::from_millis(120)); // an in-flight bar may still land
        let frozen = bars(&dep);
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(bars(&dep), frozen, "no bars while paused");
        assert_eq!(dep.status(), Status::Paused);
        m.resume(&s.id).unwrap();
        wait_until("progress after resume", || bars(&dep) > frozen + 2);
        let kinds: Vec<String> = dep.detail(&Cursor::default()).events.into_iter().map(|e| e.kind).collect();
        assert!(kinds.contains(&"paused".to_string()) && kinds.contains(&"resumed".to_string()));
        m.shutdown();
    }

    #[test]
    fn stop_can_flatten_first_and_is_final() {
        let dir = tempfile::tempdir().unwrap();
        let m = Manager::open(dir.path()).unwrap();
        let s = m.deploy(request(ALWAYS_BUY, sim_feed(20))).unwrap();
        let dep = m.get(&s.id).unwrap();
        wait_until("a position", || !dep.detail(&Cursor::default()).positions.is_empty());
        dep.stop(true).unwrap();
        let d = dep.detail(&Cursor::default());
        assert_eq!(d.summary.status, Status::Stopped);
        assert!(d.positions.is_empty(), "stop(flatten) closes positions");
        assert!(d.events.iter().any(|e| e.kind == "flatten"));
        assert!(d.trades.iter().any(|t| t.process == "manual" && t.side == "sell"));
        assert!(dep.resume().is_err() && dep.flatten().is_err(), "a stopped deployment is final");
        let frozen = d.summary.bars;
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(bars(&dep), frozen);
        // Stopping again is harmless.
        dep.stop(false).unwrap();
        m.shutdown();
    }

    #[test]
    fn stop_without_flatten_leaves_positions_open() {
        let dir = tempfile::tempdir().unwrap();
        let m = Manager::open(dir.path()).unwrap();
        let s = m.deploy(request(ALWAYS_BUY, sim_feed(20))).unwrap();
        let dep = m.get(&s.id).unwrap();
        wait_until("a position", || !dep.detail(&Cursor::default()).positions.is_empty());
        dep.stop(false).unwrap();
        assert!(!dep.detail(&Cursor::default()).positions.is_empty());
        m.shutdown();
    }

    #[test]
    fn the_drawdown_guard_closes_positions_pauses_and_does_not_repeat() {
        let dir = tempfile::tempdir().unwrap();
        let m = Manager::open(dir.path()).unwrap();
        let prices: Vec<f64> = [100.0, 100.0, 100.0, 90.0, 85.0, 80.0].iter().copied().chain(std::iter::repeat(80.0).take(14)).collect();
        let mut req = request(THRESHOLD, csv_feed(&prices, 20));
        req.params = serde_json::from_value(serde_json::json!({"floor": 1000, "ceiling": 1e9, "invest": 90_000})).unwrap();
        req.max_drawdown_pct = Some(5.0);
        let s = m.deploy(req).unwrap();
        let dep = m.get(&s.id).unwrap();
        wait_until("the guard to pause the deployment", || dep.status() == Status::Paused);
        let d = dep.detail(&Cursor::default());
        assert!(d.summary.reason.contains("drawdown limit"), "{}", d.summary.reason);
        assert!(d.positions.is_empty(), "positions were closed");
        assert!(d.events.iter().any(|e| e.kind == "risk" && e.message.contains("max drawdown 5%")));
        // Resuming does not retrigger while the drawdown stays above the limit.
        m.resume(&s.id).unwrap();
        wait_until("replay to finish", || dep.status() == Status::Finished);
        let risk_events = dep.detail(&Cursor::default()).events.iter().filter(|e| e.kind == "risk").count();
        assert_eq!(risk_events, 1);
        m.shutdown();
    }

    #[test]
    fn a_strategy_that_fails_ends_in_error_with_the_reason() {
        let dir = tempfile::tempdir().unwrap();
        let m = Manager::open(dir.path()).unwrap();
        let src = "machine M { param sym: Symbol; init { launch (p, A); } process p { state A: goto A { record x in y; } } }";
        let s = m.deploy(request(src, sim_feed(20))).unwrap();
        let dep = m.get(&s.id).unwrap();
        wait_until("the error", || dep.status() == Status::Error);
        let d = dep.detail(&Cursor::default());
        assert!(d.summary.reason.contains("not simulated"), "{}", d.summary.reason);
        assert!(d.events.iter().any(|e| e.kind == "error"));
        m.shutdown();
    }

    #[test]
    fn deploying_validates_everything_up_front() {
        let dir = tempfile::tempdir().unwrap();
        let m = Manager::open(dir.path()).unwrap();
        let bad = |mutate: &dyn Fn(&mut DeployRequest)| {
            let mut r = request(THRESHOLD, sim_feed(20));
            mutate(&mut r);
            match m.deploy(r) {
                Err(LiveError::Invalid(msg)) => msg,
                other => panic!("expected Invalid, got {other:?}"),
            }
        };
        assert!(bad(&|r| r.name = "  ".into()).contains("name"));
        assert!(bad(&|r| r.source = "machine M {\n init { launch(p, A) }\n}".into()).contains("syntax error on line 2"));
        assert!(bad(&|r| r.source = "machine M { init { launch (p, A); } process p { state A: goto Z; } }".into()).contains("goto Z"));
        assert!(bad(&|r| r.feed = FeedSpec::Simulated { symbol: "a b".into(), generator: Default::default(), tick_ms: 20 }).contains("invalid symbol"));
        assert!(bad(&|r| r.max_drawdown_pct = Some(0.0)).contains("max_drawdown_pct"));
        assert!(bad(&|r| r.initial_cash = -5.0).contains("initial_cash"));
        assert!(bad(&|r| r.machine = Some("Nope".into())).contains("Nope"));
        assert!(bad(&|r| r.max_position_value = Some(-1.0)).contains("max_position_value"));
        assert!(m.list().is_empty(), "nothing is stored for a rejected deployment");
        let folders = std::fs::read_dir(dir.path().join(".live")).unwrap().flatten().filter(|e| e.path().is_dir()).count();
        assert_eq!(folders, 0, "no deployment folder is left behind by a rejected request");
    }

    #[test]
    fn deleting_requires_a_final_status_and_removes_the_files() {
        let dir = tempfile::tempdir().unwrap();
        let m = Manager::open(dir.path()).unwrap();
        let s = m.deploy(request(THRESHOLD, sim_feed(20))).unwrap();
        assert!(matches!(m.delete(&s.id), Err(LiveError::Conflict(_))));
        m.get(&s.id).unwrap().stop(false).unwrap();
        m.delete(&s.id).unwrap();
        assert!(m.list().is_empty());
        assert!(!dir.path().join(".live").join(&s.id).exists());
        assert!(matches!(m.get(&s.id), Err(LiveError::NotFound)));
        m.shutdown();
    }

    #[test]
    fn a_restart_restores_the_exact_state_and_resumes() {
        let dir = tempfile::tempdir().unwrap();
        let m1 = Manager::open(dir.path()).unwrap();
        let s = m1.deploy(request(THRESHOLD, sim_feed(20))).unwrap();
        let dep = m1.get(&s.id).unwrap();
        wait_until("enough bars for trades", || bars(&dep) >= 70 && dep.detail(&Cursor::default()).trade_count >= 2);
        m1.shutdown();
        let before = dep.detail(&Cursor::default());
        assert_eq!(before.summary.status, Status::Running, "a clean shutdown keeps the status");
        let n = before.summary.bars;

        let m2 = Manager::open(dir.path()).unwrap();
        let again = m2.get(&s.id).unwrap();
        let restored = again.detail(&Cursor::default());
        assert_eq!(restored.summary.status, Status::Running);
        assert!(restored.summary.bars >= n);
        assert_eq!(restored.series.equity[..n], before.series.equity[..n], "the equity curve is reproduced exactly");
        assert_eq!(restored.trades[..before.trade_count], before.trades[..], "the same trades");
        assert_eq!(restored.processes.len(), before.processes.len());
        assert!(restored.events.iter().any(|e| e.kind == "restored"), "the restart is in the audit trail");
        assert_eq!(restored.summary.name, "test deployment");
        // And it keeps going.
        let at = restored.summary.bars;
        wait_until("progress after the restart", || bars(&again) > at + 3);
        // The synthetic stream continues exactly where it left off (no repeated or skipped bars).
        let all = again.detail(&Cursor { bars_from: Some(0), ..Default::default() });
        assert_eq!(all.series.close[..n], before.series.close[..n]);
        m2.shutdown();
    }

    #[test]
    fn manual_flatten_is_part_of_the_replayed_history() {
        let dir = tempfile::tempdir().unwrap();
        let m1 = Manager::open(dir.path()).unwrap();
        let s = m1.deploy(request(ALWAYS_BUY, sim_feed(20))).unwrap();
        let dep = m1.get(&s.id).unwrap();
        wait_until("a position", || bars(&dep) >= 4);
        dep.pause().unwrap();
        std::thread::sleep(Duration::from_millis(100));
        dep.flatten().unwrap();
        let before = dep.detail(&Cursor::default());
        assert!(before.positions.is_empty());
        let cash_before = before.cash;
        m1.shutdown();

        let m2 = Manager::open(dir.path()).unwrap();
        let again = m2.get(&s.id).unwrap().detail(&Cursor::default());
        assert_eq!(again.summary.status, Status::Paused);
        assert!(again.positions.is_empty(), "the flatten was replayed");
        assert!((again.cash - cash_before).abs() < 1e-9);
        assert_eq!(again.trade_count, before.trade_count);
        m2.shutdown();
    }

    #[test]
    fn a_final_deployment_is_restored_without_running() {
        let dir = tempfile::tempdir().unwrap();
        let m1 = Manager::open(dir.path()).unwrap();
        let s = m1.deploy(request(THRESHOLD, csv_feed(&[100.0, 101.0, 102.0], 20))).unwrap();
        let dep = m1.get(&s.id).unwrap();
        wait_until("finish", || dep.status() == Status::Finished);
        m1.shutdown();
        let m2 = Manager::open(dir.path()).unwrap();
        let d = m2.get(&s.id).unwrap();
        assert_eq!(d.status(), Status::Finished);
        assert_eq!(d.summary().bars, 3);
        m2.shutdown();
    }

    #[test]
    fn repeated_feed_failures_pause_the_deployment() {
        let dir = tempfile::tempdir().unwrap();
        let m = Manager::open(dir.path()).unwrap();
        let s = m.deploy(request(THRESHOLD, FeedSpec::Simulated { symbol: "TEST".into(), generator: Default::default(), tick_ms: 3_600_000 })).unwrap();
        let dep = m.get(&s.id).unwrap();
        for i in 0..MAX_FEED_FAILURES {
            assert_eq!(dep.status(), Status::Running, "still running after {i} failures");
            dep.feed_failed("boom".into());
        }
        assert_eq!(dep.status(), Status::Paused);
        let d = dep.detail(&Cursor::default());
        assert_eq!(d.feed_health.consecutive_failures, MAX_FEED_FAILURES);
        assert_eq!(d.feed_health.last_error.as_deref(), Some("boom"));
        assert!(d.summary.reason.contains("feed failures"));
        // Resuming clears the counter.
        dep.resume().unwrap();
        assert_eq!(dep.detail(&Cursor::default()).feed_health.consecutive_failures, 0);
        m.shutdown();
    }

    #[test]
    fn limits_can_be_changed_while_running_and_survive_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let m1 = Manager::open(dir.path()).unwrap();
        let mut req = request(ALWAYS_BUY, sim_feed(3_600_000));
        req.start = false;
        let s = m1.deploy(req).unwrap();
        assert_eq!(s.status, Status::Paused);
        let dep = m1.get(&s.id).unwrap();
        dep.set_limits(Some(Some(12.5)), Some(Some(250.0))).unwrap();
        let l = dep.detail(&Cursor::default()).limits;
        assert_eq!((l.max_drawdown_pct, l.max_position_value), (Some(12.5), Some(250.0)));
        assert!(dep.set_limits(Some(Some(500.0)), None).is_err());
        assert!(dep.set_limits(None, Some(Some(-1.0))).is_err());
        dep.set_limits(Some(None), None).unwrap();
        assert_eq!(dep.detail(&Cursor::default()).limits.max_drawdown_pct, None);
        m1.shutdown();
        let m2 = Manager::open(dir.path()).unwrap();
        let l = m2.get(&s.id).unwrap().detail(&Cursor::default()).limits;
        assert_eq!((l.max_drawdown_pct, l.max_position_value), (None, Some(250.0)), "the position cap is replayed from the log");
        m2.shutdown();
    }

    #[test]
    fn the_event_log_on_disk_matches_what_the_ui_sees() {
        let dir = tempfile::tempdir().unwrap();
        let m = Manager::open(dir.path()).unwrap();
        let s = m.deploy(request(THRESHOLD, csv_feed(&[100.0, 90.0, 90.0, 120.0, 120.0], 20))).unwrap();
        let dep = m.get(&s.id).unwrap();
        wait_until("finish", || dep.status() == Status::Finished);
        let ui = dep.detail(&Cursor::default()).events;
        let text = std::fs::read_to_string(dir.path().join(".live").join(&s.id).join("events.jsonl")).unwrap();
        let disk: Vec<Event> = text.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(disk.len(), ui.len());
        assert_eq!(disk.iter().map(|e| e.seq).collect::<Vec<_>>(), ui.iter().map(|e| e.seq).collect::<Vec<_>>());
        m.shutdown();
    }

    #[test]
    fn two_managers_cannot_share_a_workspace_until_the_first_shuts_down() {
        let dir = tempfile::tempdir().unwrap();
        let m1 = Manager::open(dir.path()).unwrap();
        let err = Manager::open(dir.path()).err().expect("the second open must fail");
        assert!(err.contains("already running"), "{err}");
        m1.shutdown();
        let m2 = Manager::open(dir.path()).expect("the lock is released by shutdown");
        m2.shutdown();
    }

    #[test]
    fn a_torn_last_log_line_does_not_prevent_recovery() {
        let dir = tempfile::tempdir().unwrap();
        let m1 = Manager::open(dir.path()).unwrap();
        let s = m1.deploy(request(THRESHOLD, sim_feed(20))).unwrap();
        let dep = m1.get(&s.id).unwrap();
        wait_until("bars", || bars(&dep) >= 5);
        m1.shutdown();
        let n = bars(&dep);
        let log = dir.path().join(".live").join(&s.id).join("log.jsonl");
        let mut f = OpenOptions::new().append(true).open(&log).unwrap();
        write!(f, "{{\"t\":\"bar\",\"bars\":{{\"TEST\":{{\"lab").unwrap(); // a write cut off by a crash
        let m2 = Manager::open(dir.path()).unwrap();
        assert!(m2.get(&s.id).unwrap().summary().bars >= n);
        m2.shutdown();
    }

    // ---- deployments that trade through a brokerage -----------------------------

    mod brokerage {
        use super::*;
        use crate::brokers::{tests::workspace_with, Brokers, EnvFn};
        use ezt_broker::fake::{FakeServer, Flavor};

        const BRACKET: &str = include_str!("../../../examples/bracket.sm");

        fn setup() -> (FakeServer, tempfile::TempDir, Arc<Manager>) {
            let fake = FakeServer::start(Flavor::Alpaca, "key-123", "", 50_000.0);
            fake.price("ACME", 100.0);
            let (dir, brokers) = workspace_with("alpaca-paper.json", &fake.url());
            let m = Manager::open_with(dir.path(), Arc::new(brokers)).unwrap();
            (fake, dir, m)
        }

        fn broker_request(source: &str, limits: bk::Limits) -> DeployRequest {
            let feed = FeedSpec::Broker { broker: "alpaca-paper".into(), symbols: vec!["ACME".into()], tick_ms: 250 };
            let mut r = request(source, feed);
            r.execution = Execution::Broker { broker: "alpaca-paper".into(), limits, confirm: None };
            r
        }

        fn posts(fake: &FakeServer) -> usize {
            fake.requests().iter().filter(|r| r.starts_with("POST")).count()
        }

        fn held(fake: &FakeServer) -> f64 {
            fake.positions().iter().map(|p| p.qty).sum()
        }

        #[test]
        fn a_strategy_trades_through_the_brokerage_and_the_account_is_the_source_of_truth() {
            let (fake, _dir, m) = setup();
            let s = m.deploy(broker_request(THRESHOLD, bk::Limits::default())).unwrap();
            assert_eq!((s.execution.as_str(), s.environment), ("alpaca-paper", Some(bk::Environment::Paper)));
            assert_eq!(s.initial_cash, 50_000.0, "returns are measured against the real account");
            let dep = m.get(&s.id).unwrap();
            wait_until("first bars", || bars(&dep) >= 2);
            assert_eq!(posts(&fake), 0);
            fake.price("ACME", 90.0);
            wait_until("the buy to reach the brokerage", || held(&fake) > 0.0);
            assert!((held(&fake) - 500.0 / 90.0).abs() < 1e-6, "{}", held(&fake));
            wait_until("the position to show up in the deployment", || !dep.detail(&Cursor::default()).positions.is_empty());
            let d = dep.detail(&Cursor::default());
            assert_eq!(d.orders[0].state, bk::OrderState::Filled);
            assert!(d.orders[0].client_id.starts_with(&format!("{}-", s.id)));
            assert!(d.events.iter().any(|e| e.kind == "order" && e.message.starts_with("BUY")));
            assert_eq!(d.broker.as_ref().unwrap().name, "alpaca-paper");
            assert!(d.broker.as_ref().unwrap().account.is_some());
            fake.price("ACME", 110.0);
            wait_until("the sale", || held(&fake) == 0.0);
            wait_until("the profit to show", || dep.detail(&Cursor::default()).stats.realized_pnl > 0.0);
            assert!(fake.cash() > 50_000.0);
            m.shutdown();
        }

        #[test]
        fn after_a_restart_the_strategy_is_rebuilt_from_its_log_without_sending_anything_again() {
            let (fake, dir, m) = setup();
            let s = m.deploy(broker_request(THRESHOLD, bk::Limits::default())).unwrap();
            let dep = m.get(&s.id).unwrap();
            wait_until("first bars", || bars(&dep) >= 2);
            fake.price("ACME", 90.0);
            wait_until("the buy", || held(&fake) > 0.0);
            wait_until("the HOLD state", || dep.summary().processes.iter().any(|p| p.state == "HOLD"));
            let (posts_before, shares) = (posts(&fake), held(&fake));
            m.shutdown();
            drop(dep);
            drop(m);

            let brokers = Brokers::new(dir.path(), false, Arc::new(|n: &str| match n {
                "ALPACA_KEY_ID" => Some("key-123".to_owned()),
                "ALPACA_SECRET_KEY" => Some("secret-456".to_owned()),
                _ => None,
            }));
            let m = Manager::open_with(dir.path(), Arc::new(brokers)).unwrap();
            let dep = m.get(&s.id).unwrap();
            let d = dep.detail(&Cursor::default());
            assert_eq!(d.positions.len(), 1, "the position came back");
            assert!((d.positions[0].shares - shares).abs() < 1e-9);
            assert_eq!(d.processes[0].state, "HOLD");
            assert_eq!(posts(&fake), posts_before, "restoring placed nothing");
            assert_eq!(dep.status(), Status::Running);
            fake.price("ACME", 110.0);
            wait_until("the sale after the restart", || held(&fake) == 0.0);
            assert_eq!(posts(&fake), posts_before + 1, "exactly one new order");
            m.shutdown();
        }

        #[test]
        fn a_crash_between_sending_an_order_and_logging_it_does_not_double_the_order() {
            let (fake, dir, m) = setup();
            let s = m.deploy(broker_request(THRESHOLD, bk::Limits::default())).unwrap();
            let dep = m.get(&s.id).unwrap();
            wait_until("first bars", || bars(&dep) >= 2);
            fake.price("ACME", 90.0);
            wait_until("the buy", || held(&fake) > 0.0);
            wait_until("it to be recorded", || !dep.detail(&Cursor::default()).positions.is_empty());
            let shares = held(&fake);
            m.shutdown();
            drop(dep);
            drop(m);

            // simulate the crash: the log ends just before the brokerage's answer to the order was written
            let path = dir.path().join(".live").join(&s.id).join("log.jsonl");
            let text = std::fs::read_to_string(&path).unwrap();
            let cut = text.lines().position(|l| l.contains("\"op\":\"place_order\"")).expect("an order was logged");
            let kept: Vec<&str> = text.lines().take(cut).collect();
            std::fs::write(&path, kept.join("\n") + "\n").unwrap();

            let (_d, brokers) = (0, Brokers::new(dir.path(), false, Arc::new(|n: &str| match n {
                "ALPACA_KEY_ID" => Some("key-123".to_owned()),
                "ALPACA_SECRET_KEY" => Some("secret-456".to_owned()),
                _ => None,
            })));
            let m = Manager::open_with(dir.path(), Arc::new(brokers)).unwrap();
            let dep = m.get(&s.id).unwrap();
            wait_until("the position to come back", || !dep.detail(&Cursor::default()).positions.is_empty());
            assert!((held(&fake) - shares).abs() < 1e-9, "the brokerage still has exactly one order's worth: {}", held(&fake));
            let d = dep.detail(&Cursor::default());
            assert!((d.positions[0].shares - shares).abs() < 1e-9);
            assert_eq!(dep.status(), Status::Running);
            m.shutdown();
        }

        #[test]
        fn a_limit_stops_an_order_without_stopping_the_strategy() {
            let (fake, _dir, m) = setup();
            let limits = bk::Limits { max_order_value: Some(10.0), ..Default::default() };
            let s = m.deploy(broker_request(THRESHOLD, limits)).unwrap();
            let dep = m.get(&s.id).unwrap();
            wait_until("first bars", || bars(&dep) >= 2);
            fake.price("ACME", 90.0);
            wait_until("the refusal", || dep.detail(&Cursor::default()).events.iter().any(|e| e.kind == "risk" && e.message.contains("order value")));
            assert_eq!(held(&fake), 0.0);
            assert_eq!(posts(&fake), 0, "nothing reached the brokerage");
            assert_eq!(dep.status(), Status::Running);
            m.shutdown();
        }

        #[test]
        fn the_emergency_stop_cancels_working_orders_pauses_and_blocks_resuming_until_reset() {
            let (fake, _dir, m) = setup();
            let s = m.deploy(broker_request(BRACKET, bk::Limits::default())).unwrap();
            let dep = m.get(&s.id).unwrap();
            wait_until("a resting limit order", || dep.detail(&Cursor::default()).broker.map_or(0, |b| b.working_orders) == 1);
            assert_eq!(posts(&fake), 1);
            let paused = m.emergency_stop();
            assert_eq!(paused, [s.id.clone()]);
            assert_eq!(dep.status(), Status::Paused);
            let d = dep.detail(&Cursor::default());
            assert_eq!(d.broker.as_ref().unwrap().working_orders, 0);
            assert!(d.broker.as_ref().unwrap().kill_switch);
            assert!(fake.requests().iter().any(|r| r.starts_with("DELETE")), "the order was cancelled at the brokerage");
            let e = m.resume(&s.id).unwrap_err();
            assert!(e.to_string().contains("kill switch"), "{e}");
            m.brokers().kill_switch().reset();
            assert_eq!(m.resume(&s.id).unwrap().status, Status::Running);
            m.shutdown();
        }

        #[test]
        fn a_brokerage_outage_stops_the_deployment_with_a_clear_error() {
            let (fake, _dir, m) = setup();
            // prices from a file, orders and the account through the (broken) brokerage
            let mut r = broker_request(THRESHOLD, bk::Limits::default());
            r.feed = csv_feed(&[100.0, 100.0, 90.0, 90.0], 100);
            fake.fail_next(1000);
            let s = m.deploy(r);
            // the probe at deploy time already notices
            assert!(s.unwrap_err().to_string().contains("cannot read the account"));
            fake.fail_next(0);
            let mut r = broker_request(THRESHOLD, bk::Limits::default());
            r.feed = csv_feed(&[100.0, 100.0, 90.0, 90.0, 90.0, 90.0], 300);
            let s = m.deploy(r).unwrap();
            let dep = m.get(&s.id).unwrap();
            wait_until("first bar", || bars(&dep) >= 1);
            fake.fail_next(1000);
            wait_until("the error", || dep.status() == Status::Error);
            let d = dep.detail(&Cursor::default());
            assert!(d.summary.reason.contains("broker:"), "{}", d.summary.reason);
            assert!(d.events.iter().any(|e| e.kind == "error"));
            m.shutdown();
        }

        #[test]
        fn without_credentials_after_a_restart_the_deployment_waits_paused_and_says_why() {
            let (fake, dir, m) = setup();
            let s = m.deploy(broker_request(THRESHOLD, bk::Limits::default())).unwrap();
            let dep = m.get(&s.id).unwrap();
            wait_until("first bars", || bars(&dep) >= 2);
            fake.price("ACME", 90.0);
            wait_until("the buy", || held(&fake) > 0.0);
            wait_until("it to be recorded", || !dep.detail(&Cursor::default()).positions.is_empty());
            m.shutdown();
            drop(dep);
            drop(m);
            let no_env: EnvFn = Arc::new(|_| None);
            let m = Manager::open_with(dir.path(), Arc::new(Brokers::new(dir.path(), false, no_env))).unwrap();
            let dep = m.get(&s.id).unwrap();
            assert_eq!(dep.status(), Status::Paused);
            let d = dep.detail(&Cursor::default());
            assert_eq!(d.positions.len(), 1, "state is still restored from the log");
            assert!(d.summary.reason.contains("ALPACA_KEY_ID"), "{}", d.summary.reason);
            assert!(m.resume(&s.id).is_err());
            m.shutdown();
        }

        #[test]
        fn real_money_connections_need_the_server_flag_a_typed_confirmation_limits_and_real_prices() {
            let (_f, dir, m) = setup();
            let mut v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.path().join("brokers/alpaca-paper.json")).unwrap()).unwrap();
            v["name"] = "real".into();
            v["environment"] = "live".into();
            v["base_url"] = "https://broker.invalid".into();
            v["limits"] = serde_json::json!({});
            std::fs::write(dir.path().join("brokers/real.json"), v.to_string()).unwrap();
            let live = |confirm: Option<&str>, limits: bk::Limits, feed: FeedSpec| {
                let mut r = request(THRESHOLD, feed);
                r.execution = Execution::Broker { broker: "real".into(), limits, confirm: confirm.map(str::to_owned) };
                r
            };
            let quotes = || FeedSpec::Broker { broker: "real".into(), symbols: vec!["ACME".into()], tick_ms: 1000 };
            let lim = bk::Limits { max_order_value: Some(10.0), max_position_value: Some(50.0), ..Default::default() };
            let e = m.deploy(live(Some("real"), lim.clone(), quotes())).unwrap_err().to_string();
            assert!(e.contains("--allow-live"), "{e}");
            m.shutdown();

            let env: EnvFn = Arc::new(|_| Some("value1234".into()));
            let m = Manager::open_with(dir.path(), Arc::new(Brokers::new(dir.path(), true, env))).unwrap();
            assert!(m.deploy(live(None, lim.clone(), quotes())).unwrap_err().to_string().contains("type its name"));
            assert!(m.deploy(live(Some("real"), bk::Limits::default(), quotes())).unwrap_err().to_string().contains("max_order_value"));
            assert!(m.deploy(live(Some("real"), lim, csv_feed(&[1.0, 2.0], 50))).unwrap_err().to_string().contains("real prices"));
            m.shutdown();
        }
    }
}
