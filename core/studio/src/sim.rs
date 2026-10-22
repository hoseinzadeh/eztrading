//! A preview simulator for SML strategies.
//!
//! It interprets the strategy's AST over historical (or synthetic) bars and
//! mirrors the execution model of the generated C++ runtime:
//!
//! * a process wakes up on its interval (or on every bar if it `listen`s);
//! * on each wake-up it runs the actions of its **current** state, then tries the
//!   state's `goto`s in order; the first true guard changes the state, and the
//!   new state runs on the *next* wake-up;
//! * guards are plain booleans, evaluated on every wake-up.
//!
//! What it deliberately is **not**: the C++ engine. `SandBox` in the runtime
//! only counts shares; here orders fill at the bar's close against a simulated
//! cash balance so that a backtest has a P&L. Constructs the simulator cannot
//! model (inline C++, `history()`, `record`, ...) stop the run with an error
//! naming the line, rather than being silently skipped.

use crate::ast::{self, BinOp, Expr, Stmt, StmtKind};
use ezt_broker::{self as bk, Broker};
use crate::market::{self, Bar, Market};
use crate::model::{EventDef, Interval, Machine, Process, Strategy};
use crate::syntax::{Rule, SmlParser};
use pest::Parser;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::collections::{BTreeMap, HashMap};

const MAX_STEPS: u64 = 50_000_000;
const MAX_LOOP: usize = 10_000;
const MAX_DEPTH: usize = 32;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SimError {
    pub line: Option<usize>,
    pub bar: Option<usize>,
    pub message: String,
}

impl std::fmt::Display for SimError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(l) = self.line {
            write!(f, "line {l}: ")?;
        }
        write!(f, "{}", self.message)?;
        if let Some(b) = self.bar {
            write!(f, " (bar {b})")?;
        }
        Ok(())
    }
}

/// Internal failure: a real error, or "not enough history yet" which just
/// skips the current wake-up.
enum Fail {
    Err(SimError),
    NoHistory,
}

type R<T> = Result<T, Fail>;

#[derive(Debug, Clone, PartialEq)]
enum Value {
    Unit,
    Num(f64),
    Bool(bool),
    Str(String),
    Dur(f64),
    Pos { sym: String, shares: f64 },
    Tick { sym: String, off: i64 },
    Tickers,
    Account,
    /// A handle to an order placed through the account (its id).
    Order(String),
    List(Vec<Value>),
}

impl Value {
    fn kind(&self) -> &'static str {
        match self {
            Value::Unit => "an uninitialised value",
            Value::Num(_) => "a number",
            Value::Bool(_) => "a boolean",
            Value::Str(_) => "a string",
            Value::Dur(_) => "a duration",
            Value::Pos { .. } => "a position",
            Value::Tick { .. } => "a ticker",
            Value::Tickers => "`Tickers`",
            Value::Account => "an account",
            Value::Order(_) => "an order",
            Value::List(_) => "a list",
        }
    }

    fn show(&self) -> String {
        match self {
            Value::Num(n) => format!("{n}"),
            Value::Bool(b) => b.to_string(),
            Value::Str(s) => s.clone(),
            Value::Dur(d) => ast::show_duration(*d),
            Value::Pos { sym, shares } => format!("{shares} {sym}"),
            Value::Order(id) => format!("order {id}"),
            other => other.kind().to_owned(),
        }
    }
}

#[derive(Default)]
struct Env {
    scopes: Vec<HashMap<String, Value>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    pub machine: Option<String>,
    /// Parameter overrides: numbers, booleans, strings (`"5 min"`, `"AAPL"`) or arrays.
    pub params: BTreeMap<String, serde_json::Value>,
    pub initial_cash: f64,
    /// Trading cost per fill, in basis points of the traded value.
    pub fee_bps: f64,
    pub risk: Risk,
    /// Prefix of the client order ids (a deployment uses its own id, so ids are unique per deployment).
    #[serde(skip)]
    pub client_prefix: Option<String>,
}

/// Guards enforced by the engine itself.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct Risk {
    /// A buy that would take a position above this value (at the fill price) is reduced.
    pub max_position_value: Option<f64>,
}

impl Default for Config {
    fn default() -> Self {
        Self { machine: None, params: BTreeMap::new(), initial_cash: 100_000.0, fee_bps: 0.0, risk: Risk::default(), client_prefix: None }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Trade {
    pub bar: usize,
    pub label: String,
    pub symbol: String,
    pub side: &'static str,
    pub shares: f64,
    pub price: f64,
    pub process: String,
    pub state: String,
    /// Realised profit of a sell (after fees), against the average cost.
    pub pnl: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Timeline {
    pub process: String,
    pub states: Vec<String>,
    /// State index (into `states`) the process was in when it woke on each bar.
    pub series: Vec<usize>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Stats {
    pub bars: usize,
    pub initial_cash: f64,
    pub final_equity: f64,
    pub total_return_pct: f64,
    pub buy_hold_return_pct: f64,
    pub max_drawdown_pct: f64,
    pub fills: usize,
    pub closed_trades: usize,
    pub win_rate_pct: Option<f64>,
    pub realized_pnl: f64,
    pub exposure_pct: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Backtest {
    pub machine: String,
    pub symbol: String,
    pub bars: Vec<Bar>,
    pub equity: Vec<f64>,
    pub buy_hold: Vec<f64>,
    pub drawdown_pct: Vec<f64>,
    /// Shares held of the primary symbol after each bar.
    pub position: Vec<f64>,
    pub trades: Vec<Trade>,
    pub timelines: Vec<Timeline>,
    pub stats: Stats,
    pub warnings: Vec<String>,
    pub logs: Vec<String>,
    /// Final state of every process instance.
    pub final_states: BTreeMap<String, String>,
}

struct Holding {
    shares: f64,
    avg_cost: f64,
}

struct Instance {
    name: String,
    proc_idx: usize,
    state: usize,
    env: Env,
    started: bool,
    every: usize,
    wakes_every_bar: bool,
}

/// Mutable simulation data that lives between steps (so a run can be paused,
/// resumed and inspected). `Interp` only borrows the immutable parts.
#[derive(Default)]
struct State {
    mvars: HashMap<String, Value>,
    bar: usize,
    /// Fee rate used only to size buys (the paper broker charges it; real brokers report their own).
    fee: f64,
    holdings: BTreeMap<String, Holding>,
    broker: Option<Box<dyn Broker>>,
    tracker: bk::FillTracker,
    /// Orders that are still working at the broker.
    working: Vec<String>,
    /// Latest snapshot of every order the strategy can hold a handle to.
    orders: HashMap<String, bk::Order>,
    seq: u64,
    prefix: String,
    account: Option<bk::Account>,
    /// Order ids, oldest first (for display).
    recent: std::collections::VecDeque<String>,
    /// Orders sent during the current step, described for the activity feed.
    placed: Vec<String>,
    realized: f64,
    closes: usize,
    wins: usize,
    trades: Vec<Trade>,
    logs: Vec<String>,
    warnings: Vec<String>,
    warned_cash: bool,
    steps: u64,
    line: usize,
    depth: usize,
    cur_process: String,
    cur_state: String,
    risk: Risk,
    /// Risk-guard actions of the current step (drained into the step report).
    notes: Vec<String>,
}

struct Interp<'a> {
    machine: &'a Machine,
    market: &'a Market,
    events: HashMap<&'a str, &'a EventDef>,
    st: State,
}

enum Flow {
    Normal,
    Return(Value),
}

impl<'a> Interp<'a> {
    fn err<T>(&self, msg: impl Into<String>) -> R<T> {
        Err(Fail::Err(SimError { line: Some(self.st.line).filter(|l| *l > 0), bar: Some(self.st.bar), message: msg.into() }))
    }

    // ---- variables -----------------------------------------------------

    fn lookup(&self, env: &Env, name: &str) -> R<Value> {
        for s in env.scopes.iter().rev() {
            if let Some(v) = s.get(name) {
                return Ok(v.clone());
            }
        }
        if let Some(v) = self.st.mvars.get(name) {
            return Ok(v.clone());
        }
        match name {
            "true" => Ok(Value::Bool(true)),
            "false" => Ok(Value::Bool(false)),
            "Tickers" => Ok(Value::Tickers),
            "SandBox" | "Broker" => Ok(Value::Account),
            _ => self.err(format!("unknown name `{name}` (not a parameter, variable or standard-library item)")),
        }
    }

    fn assign(&mut self, env: &mut Env, name: &str, v: Value) -> R<()> {
        for s in env.scopes.iter_mut().rev() {
            if let Some(slot) = s.get_mut(name) {
                *slot = v;
                return Ok(());
            }
        }
        if let Some(slot) = self.st.mvars.get_mut(name) {
            *slot = v;
            return Ok(());
        }
        self.err(format!("cannot assign to `{name}`: it is not declared"))
    }

    fn default_for(&self, ty: Option<&str>) -> Value {
        match ty.map(str::trim) {
            Some("bool") => Value::Bool(false),
            Some("Position") => Value::Pos { sym: String::new(), shares: 0.0 },
            Some("Symbol") => Value::Str(String::new()),
            Some("duration") => Value::Dur(0.0),
            Some("Account") => Value::Account,
            Some(t) if t.starts_with("Vec") => Value::List(vec![]),
            Some(t) if t.starts_with("int") || t.starts_with("uint") || t == "float" => Value::Num(0.0),
            _ => Value::Unit,
        }
    }

    // ---- coercions -----------------------------------------------------

    fn close_at(&self, sym: &str, off: i64) -> R<f64> {
        let Some(bars) = self.market.series.get(sym) else {
            return self.err(format!("no price data for symbol `{sym}`"));
        };
        let idx = self.st.bar as i64 + off;
        if idx < 0 {
            return Err(Fail::NoHistory);
        }
        if idx > self.st.bar as i64 {
            return self.err("a strategy cannot look into the future");
        }
        Ok(bars[idx as usize].close)
    }

    fn tick_field(&self, sym: &str, off: i64, field: &str) -> R<f64> {
        let Some(bars) = self.market.series.get(sym) else {
            return self.err(format!("no price data for symbol `{sym}`"));
        };
        let idx = self.st.bar as i64 + off;
        if idx < 0 {
            return Err(Fail::NoHistory);
        }
        if idx > self.st.bar as i64 {
            return self.err("a strategy cannot look into the future");
        }
        let b = &bars[idx as usize];
        match field {
            "close" => Ok(b.close),
            "open" => Ok(b.open),
            "high" => Ok(b.high),
            "low" => Ok(b.low),
            "volume" => Ok(b.volume),
            other => self.err(format!("a ticker has no field `{other}` (close, open, high, low, volume)")),
        }
    }

    fn num(&self, v: &Value) -> R<f64> {
        match v {
            Value::Num(n) | Value::Dur(n) => Ok(*n),
            Value::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
            Value::Pos { shares, .. } => Ok(*shares),
            Value::Tick { sym, off } => self.close_at(sym, *off),
            other => self.err(format!("expected a number but found {}", other.kind())),
        }
    }

    fn truthy(&self, v: &Value) -> R<bool> {
        match v {
            Value::Bool(b) => Ok(*b),
            Value::Num(n) => Ok(*n != 0.0),
            Value::Pos { shares, .. } => Ok(*shares != 0.0),
            other => self.err(format!("expected a condition but found {}", other.kind())),
        }
    }

    // ---- expressions ---------------------------------------------------

    fn eval(&mut self, e: &Expr, env: &mut Env) -> R<Value> {
        self.st.steps += 1;
        if self.st.steps > MAX_STEPS {
            return self.err("step limit exceeded (runaway loop?)");
        }
        Ok(match e {
            Expr::Num(n) => Value::Num(*n),
            Expr::Str(s) => Value::Str(s.clone()),
            Expr::Dur(d) => Value::Dur(*d),
            Expr::Ident(name) => self.lookup(env, name)?,
            Expr::Not(a) => {
                let v = self.eval(a, env)?;
                Value::Bool(!self.truthy(&v)?)
            }
            Expr::Neg(a) => match self.eval(a, env)? {
                Value::Dur(d) => Value::Dur(-d),
                v => Value::Num(-self.num(&v)?),
            },
            Expr::Unit(a, secs) => {
                let v = self.eval(a, env)?;
                Value::Dur(self.num(&v)? * secs)
            }
            Expr::Bin(BinOp::And, a, b) => {
                let l = self.eval(a, env)?;
                if !self.truthy(&l)? {
                    Value::Bool(false)
                } else {
                    let r = self.eval(b, env)?;
                    Value::Bool(self.truthy(&r)?)
                }
            }
            Expr::Bin(BinOp::Or, a, b) => {
                let l = self.eval(a, env)?;
                if self.truthy(&l)? {
                    Value::Bool(true)
                } else {
                    let r = self.eval(b, env)?;
                    Value::Bool(self.truthy(&r)?)
                }
            }
            Expr::Bin(op, a, b) => {
                let l = self.eval(a, env)?;
                let r = self.eval(b, env)?;
                self.binop(*op, &l, &r)?
            }
            Expr::Assign(op, target, value) => {
                let mut v = self.eval(value, env)?;
                if let Some(op) = op {
                    let cur = self.eval(target, env)?;
                    v = self.binop(*op, &cur, &v)?;
                }
                match &**target {
                    Expr::Ident(name) => self.assign(env, name, v.clone())?,
                    other => return self.err(format!("cannot assign to `{}`", ast::show(other))),
                }
                v
            }
            Expr::Cond(c, a, b) => {
                let cv = self.eval(c, env)?;
                if self.truthy(&cv)? {
                    self.eval(a, env)?
                } else {
                    self.eval(b, env)?
                }
            }
            Expr::Member(obj, field) => {
                let o = self.eval(obj, env)?;
                self.field(&o, field)?
            }
            Expr::Index(obj, args) => {
                let base = self.eval(obj, env)?;
                if args.len() != 1 {
                    return self.err("indexing takes exactly one argument");
                }
                let idx = self.eval(&args[0], env)?;
                self.index(base, idx)?
            }
            Expr::Call(callee, args) => self.call(callee, args, env)?,
            Expr::Range(..) => Value::List(self.range_items(e, env)?),
            Expr::List(items) => {
                let mut out = vec![];
                for i in items {
                    out.push(self.eval(i, env)?);
                }
                Value::List(out)
            }
            Expr::Unsupported(what) => return self.err(format!("not simulated: {what}")),
        })
    }

    fn range_items(&mut self, e: &Expr, env: &mut Env) -> R<Vec<Value>> {
        let Expr::Range(a, b, inclusive) = e else { unreachable!() };
        let lo = self.eval(a, env)?;
        let hi = self.eval(b, env)?;
        let (lo, hi) = (self.num(&lo)?.floor() as i64, self.num(&hi)?.floor() as i64);
        let hi = if *inclusive { hi + 1 } else { hi };
        if hi - lo > 1_000_000 {
            return self.err("range is too large");
        }
        Ok((lo..hi).map(|i| Value::Num(i as f64)).collect())
    }

    fn binop(&self, op: BinOp, a: &Value, b: &Value) -> R<Value> {
        use BinOp::*;
        if let (Value::Str(x), Value::Str(y)) = (a, b) {
            return match op {
                Eq => Ok(Value::Bool(x == y)),
                Ne => Ok(Value::Bool(x != y)),
                _ => self.err("strings only support `==` and `!=`"),
            };
        }
        let (x, y) = (self.num(a)?, self.num(b)?);
        let dur = |v: f64| Value::Dur(v);
        Ok(match op {
            Add | Sub => {
                let r = if op == Add { x + y } else { x - y };
                if matches!(a, Value::Dur(_)) && matches!(b, Value::Dur(_)) { dur(r) } else { Value::Num(r) }
            }
            Mul => {
                if matches!(a, Value::Dur(_)) != matches!(b, Value::Dur(_)) { dur(x * y) } else { Value::Num(x * y) }
            }
            Div | Rem => {
                if y == 0.0 {
                    return self.err("division by zero");
                }
                let r = if op == Div { x / y } else { x % y };
                if matches!(a, Value::Dur(_)) && !matches!(b, Value::Dur(_)) { dur(r) } else { Value::Num(r) }
            }
            Lt => Value::Bool(x < y),
            Le => Value::Bool(x <= y),
            Gt => Value::Bool(x > y),
            Ge => Value::Bool(x >= y),
            Eq => Value::Bool(x == y),
            Ne => Value::Bool(x != y),
            And | Or => unreachable!("short-circuit operators are handled in eval"),
        })
    }

    fn field(&self, obj: &Value, field: &str) -> R<Value> {
        match obj {
            Value::Pos { shares, .. } if field == "shares" => Ok(Value::Num(*shares)),
            Value::Tick { sym, off } => Ok(Value::Num(self.tick_field(sym, *off, field)?)),
            other => self.err(format!("{} has no field `{field}`", other.kind())),
        }
    }

    fn index(&self, base: Value, idx: Value) -> R<Value> {
        match (base, idx) {
            (Value::Tickers, Value::Str(sym)) => {
                if self.market.series.contains_key(&sym) {
                    Ok(Value::Tick { sym, off: 0 })
                } else {
                    self.err(format!("no price data for symbol `{sym}` (available: {})", self.market.symbols().join(", ")))
                }
            }
            (Value::Tick { sym, off }, Value::Dur(d)) => {
                let bars = (d / self.market.bar_secs).round() as i64;
                Ok(Value::Tick { sym, off: off + bars })
            }
            (Value::List(items), v) => {
                let i = self.num(&v)?;
                match items.get(i as usize) {
                    Some(x) if i >= 0.0 => Ok(x.clone()),
                    _ => self.err("list index out of range"),
                }
            }
            (b, i) => self.err(format!("cannot index {} with {}", b.kind(), i.kind())),
        }
    }

    // ---- calls ---------------------------------------------------------

    fn call(&mut self, callee: &Expr, args: &[Expr], env: &mut Env) -> R<Value> {
        let mut vals = Vec::with_capacity(args.len());
        match callee {
            Expr::Ident(name) => {
                for a in args {
                    vals.push(self.eval(a, env)?);
                }
                self.call_function(name, vals)
            }
            Expr::Member(obj, method) => {
                let target = self.eval(obj, env)?;
                for a in args {
                    vals.push(self.eval(a, env)?);
                }
                self.call_method(target, method, vals)
            }
            other => self.err(format!("`{}` is not callable", ast::show(other))),
        }
    }

    fn call_function(&mut self, name: &str, args: Vec<Value>) -> R<Value> {
        if let Some(ev) = self.events.get(name).copied() {
            if ev.params.len() != args.len() {
                return self.err(format!("event `{name}` takes {} argument(s), got {}", ev.params.len(), args.len()));
            }
            if self.st.depth >= MAX_DEPTH {
                return self.err("events are nested too deeply");
            }
            self.st.depth += 1;
            let saved_line = self.st.line;
            let mut env = Env { scopes: vec![ev.params.iter().cloned().zip(args).collect()] };
            let flow = self.exec_stmts(&ev.body, &mut env);
            self.st.depth -= 1;
            self.st.line = saved_line;
            return match flow? {
                Flow::Return(v) => Ok(v),
                Flow::Normal => Ok(Value::Unit),
            };
        }
        match name {
            "printf" => {
                let mut shown = Vec::with_capacity(args.len());
                for a in args {
                    shown.push(match a {
                        Value::Tick { .. } => Value::Num(self.num(&a)?),
                        other => other,
                    });
                }
                let text = self.format_printf(&shown);
                self.st.logs.push(format!("bar {}: {}", self.st.bar, text));
                Ok(Value::Unit)
            }
            "sleep" => Ok(Value::Unit),
            _ => self.err(format!("unknown function `{name}` (not an event of this strategy or a standard-library function)")),
        }
    }

    fn format_printf(&self, args: &[Value]) -> String {
        let Some(Value::Str(fmt)) = args.first() else {
            return args.iter().map(Value::show).collect::<Vec<_>>().join(" ");
        };
        let mut rest = args[1..].iter();
        let mut out = String::new();
        let mut chars = fmt.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '%' {
                match chars.next() {
                    Some('%') => out.push('%'),
                    Some(_) => out.push_str(&rest.next().map(Value::show).unwrap_or_default()),
                    None => out.push('%'),
                }
            } else {
                out.push(c);
            }
        }
        out.replace("\\n", "").trim_end().to_owned()
    }

    fn call_method(&mut self, target: Value, method: &str, args: Vec<Value>) -> R<Value> {
        match (&target, method) {
            (Value::Account, "buy" | "sell") => {
                let [Value::Str(sym), qty] = &args[..] else {
                    return self.err(format!("`account.{method}` takes (symbol, shares)"));
                };
                let qty = self.num(qty)?;
                self.fill(method == "buy", sym, qty)
            }
            (Value::Account, "buy_limit" | "sell_limit" | "buy_stop" | "sell_stop") => {
                let [Value::Str(sym), qty, level] = &args[..] else {
                    return self.err(format!("`account.{method}` takes (symbol, shares, price)"));
                };
                let (qty, level) = (self.num(qty)?, self.num(level)?);
                self.resting(method.starts_with("buy"), sym, qty, level, method.ends_with("stop"))
            }
            (Value::Account, "cancel") => {
                let [order] = &args[..] else {
                    return self.err("`account.cancel` takes (order)");
                };
                let o = self.order_ref(order)?;
                self.cancel(&o.id)
            }
            (Value::Account, "cancel_all") => {
                if !args.is_empty() {
                    return self.err("`account.cancel_all` takes no arguments");
                }
                let mut n = 0.0;
                for id in self.st.working.clone() {
                    if let Value::Bool(true) = self.cancel(&id)? {
                        n += 1.0;
                    }
                }
                Ok(Value::Num(n))
            }
            (Value::Account, "open_orders") => {
                let mut ids = self.st.working.clone();
                ids.sort();
                Ok(Value::List(ids.into_iter().map(Value::Order).collect()))
            }
            (Value::Account, "position") => {
                let [Value::Str(sym)] = &args[..] else {
                    return self.err("`account.position` takes (symbol)");
                };
                let shares = self.st.holdings.get(sym).map_or(0.0, |h| h.shares);
                Ok(Value::Pos { sym: sym.clone(), shares })
            }
            (Value::Account, "cash" | "equity" | "buying_power") => {
                if !args.is_empty() {
                    return self.err(format!("`account.{method}` takes no arguments"));
                }
                let a = self.account_now()?;
                Ok(Value::Num(match method {
                    "cash" => a.cash,
                    "equity" => a.equity,
                    _ => a.buying_power,
                }))
            }
            (Value::Account, "bid" | "ask" | "last") => {
                let [Value::Str(sym)] = &args[..] else {
                    return self.err(format!("`account.{method}` takes (symbol)"));
                };
                let q = self.quote_of(sym)?;
                Ok(Value::Num(match method {
                    "bid" => q.bid,
                    "ask" => q.ask,
                    _ => q.last,
                }))
            }
            (Value::Order(_), m @ ("status" | "filled" | "price" | "remaining" | "is_open" | "is_filled" | "cancel")) => {
                if !args.is_empty() {
                    return self.err(format!("`order.{m}` takes no arguments"));
                }
                let o = self.order_ref(&target)?;
                Ok(match m {
                    "status" => Value::Str(o.state.as_str().to_owned()),
                    "filled" => Value::Num(o.filled_qty),
                    "price" => Value::Num(o.avg_price),
                    "remaining" => Value::Num(if o.state.is_done() { 0.0 } else { (o.qty - o.filled_qty).max(0.0) }),
                    "is_open" => Value::Bool(!o.state.is_done()),
                    "is_filled" => Value::Bool(o.state == bk::OrderState::Filled),
                    _ => self.cancel(&o.id)?,
                })
            }
            (Value::Pos { sym, shares }, "value") => {
                if sym.is_empty() {
                    return Ok(Value::Num(0.0));
                }
                Ok(Value::Num(shares * self.close_at(sym, 0)?))
            }
            (Value::Tick { .. }, "history") => self.err("not simulated: `history()` (its exact semantics are defined by the C++ runtime)"),
            (t, m) => self.err(format!("{} has no method `{m}`", t.kind())),
        }
    }

    // ---- orders ----------------------------------------------------------

    fn broker_fail(&self, e: bk::BrokerError) -> Fail {
        Fail::Err(SimError {
            line: Some(self.st.line).filter(|l| *l > 0),
            bar: Some(self.st.bar),
            message: format!("broker: {e}"),
        })
    }

    fn with_broker<T>(&mut self, f: impl FnOnce(&mut dyn Broker) -> bk::Result<T>) -> R<T> {
        let Some(b) = self.st.broker.as_mut() else {
            return self.err("no broker is attached to this run");
        };
        let r = f(b.as_mut());
        r.map_err(|e| self.broker_fail(e))
    }

    fn account_now(&mut self) -> R<bk::Account> {
        let a = self.with_broker(|b| b.account())?;
        self.st.account = Some(a.clone());
        Ok(a)
    }

    fn equity(&mut self) -> R<f64> {
        Ok(self.account_now()?.equity)
    }

    fn begin_bar(&mut self, candles: &BTreeMap<String, bk::Candle>) -> R<()> {
        if let Some(b) = self.st.broker.as_mut() {
            b.on_bar(candles);
        }
        self.sync_orders()
    }

    /// Looks at every working order again and books new fills.
    fn sync_orders(&mut self) -> R<()> {
        for id in self.st.working.clone() {
            let r = match self.st.broker.as_mut() {
                Some(b) => b.order(&id),
                None => return Ok(()),
            };
            match r {
                Ok(o) => self.absorb(&o),
                Err(e) if e.kind == bk::ErrorKind::Rejected => {
                    self.st.notes.push(format!("order {id} is unknown to the broker ({e}); no longer tracked"));
                    self.st.working.retain(|w| w != &id);
                }
                Err(e) => return Err(self.broker_fail(e)),
            }
        }
        Ok(())
    }

    fn absorb(&mut self, o: &bk::Order) {
        // Replies to cancels and look-ups often leave out what we already know about the order.
        let merged;
        let o = match self.st.orders.get(&o.id) {
            Some(prev) => {
                merged = bk::Order {
                    kind: prev.kind,
                    tif: prev.tif,
                    qty: if o.qty > 0.0 { o.qty } else { prev.qty },
                    symbol: if o.symbol.is_empty() { prev.symbol.clone() } else { o.symbol.clone() },
                    client_id: if o.client_id.is_empty() { prev.client_id.clone() } else { o.client_id.clone() },
                    ..o.clone()
                };
                &merged
            }
            None => o,
        };
        if let Some(f) = self.st.tracker.update(o) {
            self.apply_fill(f);
        }
        if o.state.is_done() {
            self.st.tracker.forget_finished(o);
            self.st.working.retain(|w| w != &o.id);
            if !o.message.is_empty() {
                self.st.notes.push(format!("order {} ({} {} {}): {}", o.id, o.side.as_str(), o.qty, o.symbol, o.message));
            }
        } else if !self.st.working.contains(&o.id) {
            self.st.working.push(o.id.clone());
        }
        self.remember(o);
    }

    fn remember(&mut self, o: &bk::Order) {
        if self.st.orders.len() > 5000 {
            self.st.orders.retain(|_, x| !x.state.is_done());
            let orders = &self.st.orders;
            self.st.recent.retain(|id| orders.contains_key(id));
        }
        if self.st.orders.insert(o.id.clone(), o.clone()).is_none() {
            self.st.recent.push_back(o.id.clone());
            if self.st.recent.len() > 200 {
                self.st.recent.pop_front();
            }
        }
    }

    fn apply_fill(&mut self, f: bk::Fill) {
        let bar = self.st.bar;
        let label = self.market.series.get(&f.symbol).and_then(|s| s.get(bar)).map(|b| b.label.clone()).unwrap_or_default();
        let h = self.st.holdings.entry(f.symbol.clone()).or_insert(Holding { shares: 0.0, avg_cost: 0.0 });
        let mut pnl = None;
        let buy = f.side == bk::Side::Buy;
        if buy {
            h.avg_cost = (h.avg_cost * h.shares + f.price * f.qty) / (h.shares + f.qty);
            h.shares += f.qty;
        } else {
            let p = (f.price - h.avg_cost) * f.qty - f.fee;
            pnl = Some(p);
            self.st.realized += p;
            self.st.closes += 1;
            if p > 0.0 {
                self.st.wins += 1;
            }
            h.shares -= f.qty;
            if h.shares < 1e-12 {
                h.shares = 0.0;
                h.avg_cost = 0.0;
            }
        }
        self.st.trades.push(Trade {
            bar,
            label,
            symbol: f.symbol,
            side: if buy { "buy" } else { "sell" },
            shares: f.qty,
            price: f.price,
            process: self.st.cur_process.clone(),
            state: self.st.cur_state.clone(),
            pnl,
        });
    }

    /// Sends an order. A refusal by the broker or by a safety limit is not an
    /// error of the strategy: it becomes a rejected order the strategy can
    /// inspect, plus a note. Outages and bad credentials stop the run.
    fn place(&mut self, sym: &str, side: bk::Side, qty: f64, kind: bk::Kind) -> R<bk::Order> {
        self.st.seq += 1;
        let req = bk::OrderRequest {
            symbol: sym.to_owned(),
            side,
            qty,
            kind,
            tif: if kind == bk::Kind::Market { bk::Tif::Day } else { bk::Tif::Gtc },
            client_id: format!("{}-{}", self.st.prefix, self.st.seq),
        };
        let r = match self.st.broker.as_mut() {
            Some(b) => b.place_order(&req),
            None => return self.err("no broker is attached to this run"),
        };
        match r {
            Ok(o) => {
                self.absorb(&o);
                self.st.placed.push(format!("{} {qty} {sym} ({}) → {} [{}]", side.as_str().to_uppercase(), kind.describe(), o.state.as_str(), o.id));
                Ok(o)
            }
            Err(e) if matches!(e.kind, bk::ErrorKind::Rejected | bk::ErrorKind::Blocked) => {
                if e.kind == bk::ErrorKind::Rejected {
                    self.st.notes.push(format!("order refused: {} {qty} {sym} ({}): {}", side.as_str(), kind.describe(), e.message));
                }
                let o = bk::Order {
                    id: format!("rejected-{}", self.st.seq),
                    client_id: req.client_id,
                    symbol: sym.to_owned(),
                    side,
                    qty,
                    kind,
                    tif: req.tif,
                    state: bk::OrderState::Rejected,
                    filled_qty: 0.0,
                    avg_price: 0.0,
                    fees: 0.0,
                    message: e.message,
                };
                self.remember(&o);
                self.st.placed.push(format!("{} {qty} {sym} ({}) was refused: {}", side.as_str().to_uppercase(), kind.describe(), o.message));
                Ok(o)
            }
            Err(e) => Err(self.broker_fail(e)),
        }
    }

    fn order_ref(&self, v: &Value) -> R<bk::Order> {
        match v {
            Value::Order(id) => match self.st.orders.get(id) {
                Some(o) => Ok(o.clone()),
                None => self.err(format!("unknown order `{id}`")),
            },
            other => self.err(format!("expected an order, got {}", other.kind())),
        }
    }

    /// Sizes a buy or sell the way the strategy intends (cash, risk cap, holdings).
    fn size(&mut self, buy: bool, sym: &str, shares: f64, price: f64) -> R<f64> {
        let held = self.st.holdings.get(sym).map_or(0.0, |h| h.shares);
        if !buy {
            return Ok(shares.min(held));
        }
        let cash = self.account_now()?.buying_power;
        let unit = price * (1.0 + self.st.fee);
        let mut q = shares.min(cash / unit);
        if q < shares - 1e-9 && !self.st.warned_cash {
            self.st.warned_cash = true;
            self.st.warnings.push(format!("not enough cash: the buy of {shares:.4} {sym} on bar {} was reduced to {q:.4}", self.st.bar));
        }
        if let Some(cap) = self.st.risk.max_position_value {
            let room = ((cap - held * price) / price).max(0.0);
            if q > room + 1e-12 {
                self.st.notes.push(format!("risk limit: the buy of {q:.4} {sym} was reduced to {room:.4} (max position value {cap})"));
                q = room;
            }
        }
        Ok(q.max(0.0))
    }

    fn check_order_args(&self, sym: &str, shares: f64) -> R<f64> {
        let Some(bars) = self.market.series.get(sym) else {
            return self.err(format!("no price data for symbol `{sym}`"));
        };
        if !shares.is_finite() || shares < 0.0 {
            return self.err(format!("invalid order size {shares}"));
        }
        Ok(bars[self.st.bar].close)
    }

    /// Market order: buys are reduced to what cash and the risk cap allow, sells to what is held.
    fn fill(&mut self, buy: bool, sym: &str, shares: f64) -> R<Value> {
        let price = self.check_order_args(sym, shares)?;
        let q = self.size(buy, sym, shares, price)?;
        if q > 1e-12 {
            self.place(sym, if buy { bk::Side::Buy } else { bk::Side::Sell }, q, bk::Kind::Market)?;
        }
        let total = self.st.holdings.get(sym).map_or(0.0, |h| h.shares);
        Ok(Value::Pos { sym: sym.to_owned(), shares: total })
    }

    /// Limit or stop order (rests at the broker until it triggers or is cancelled).
    fn resting(&mut self, buy: bool, sym: &str, shares: f64, level: f64, stop: bool) -> R<Value> {
        let price = self.check_order_args(sym, shares)?;
        if !(level.is_finite() && level > 0.0) {
            return self.err(format!("invalid order price {level}"));
        }
        let q = self.size(buy, sym, shares, level.max(price))?;
        if q <= 1e-12 {
            return Ok(Value::Order(self.reject_locally(sym, buy, shares, "nothing to trade after cash, risk and holding limits")));
        }
        let kind = if stop { bk::Kind::Stop { stop: level } } else { bk::Kind::Limit { limit: level } };
        let o = self.place(sym, if buy { bk::Side::Buy } else { bk::Side::Sell }, q, kind)?;
        Ok(Value::Order(o.id))
    }

    fn reject_locally(&mut self, sym: &str, buy: bool, qty: f64, why: &str) -> String {
        self.st.seq += 1;
        let id = format!("rejected-{}", self.st.seq);
        let o = bk::Order {
                id: id.clone(),
                client_id: String::new(),
                symbol: sym.to_owned(),
                side: if buy { bk::Side::Buy } else { bk::Side::Sell },
                qty,
                kind: bk::Kind::Market,
                tif: bk::Tif::Day,
                state: bk::OrderState::Rejected,
                filled_qty: 0.0,
                avg_price: 0.0,
                fees: 0.0,
                message: why.to_owned(),
        };
        self.remember(&o);
        id
    }

    fn cancel(&mut self, id: &str) -> R<Value> {
        if id.starts_with("rejected-") {
            return Ok(Value::Bool(false));
        }
        let r = match self.st.broker.as_mut() {
            Some(b) => b.cancel_order(id),
            None => return self.err("no broker is attached to this run"),
        };
        match r {
            Ok(o) => {
                let done = o.state == bk::OrderState::Canceled;
                self.absorb(&o);
                Ok(Value::Bool(done))
            }
            Err(e) if e.kind == bk::ErrorKind::Rejected => {
                self.st.notes.push(format!("could not cancel order {id}: {}", e.message));
                Ok(Value::Bool(false))
            }
            Err(e) => Err(self.broker_fail(e)),
        }
    }

    fn quote_of(&mut self, sym: &str) -> R<bk::Quote> {
        if !self.market.series.contains_key(sym) {
            return self.err(format!("no price data for symbol `{sym}`"));
        }
        self.with_broker(|b| b.quote(sym))
    }

    // ---- statements ----------------------------------------------------

    fn exec_block(&mut self, stmts: &[Stmt], env: &mut Env) -> R<Flow> {
        env.scopes.push(HashMap::new());
        let flow = self.exec_stmts(stmts, env);
        env.scopes.pop();
        flow
    }

    fn exec_stmts(&mut self, stmts: &[Stmt], env: &mut Env) -> R<Flow> {
        for s in stmts {
            self.st.line = s.line;
            match &s.kind {
                StmtKind::Decl { name, ty, init, is_static } => {
                    if *is_static && env.scopes.first().map_or(false, |sc| sc.contains_key(name)) {
                        continue;
                    }
                    let v = match init {
                        Some(e) => self.eval(e, env)?,
                        None => self.default_for(ty.as_deref()),
                    };
                    let scope = if *is_static { env.scopes.first_mut() } else { env.scopes.last_mut() };
                    scope.expect("env always has a scope").insert(name.clone(), v);
                }
                StmtKind::Expr(e) => {
                    self.eval(e, env)?;
                }
                StmtKind::If(c, then, other) => {
                    let cv = self.eval(c, env)?;
                    let branch = if self.truthy(&cv)? { then } else { other };
                    if let Flow::Return(v) = self.exec_block(branch, env)? {
                        return Ok(Flow::Return(v));
                    }
                }
                StmtKind::While(c, body) => {
                    for n in 0.. {
                        if n >= MAX_LOOP {
                            return self.err(format!("`while` ran for more than {MAX_LOOP} iterations"));
                        }
                        self.st.line = s.line;
                        let cv = self.eval(c, env)?;
                        if !self.truthy(&cv)? {
                            break;
                        }
                        if let Flow::Return(v) = self.exec_block(body, env)? {
                            return Ok(Flow::Return(v));
                        }
                    }
                }
                StmtKind::Loop(body) => {
                    for n in 0.. {
                        if n >= MAX_LOOP {
                            return self.err(format!("`loop` ran for more than {MAX_LOOP} iterations (there is no `break`)"));
                        }
                        if let Flow::Return(v) = self.exec_block(body, env)? {
                            return Ok(Flow::Return(v));
                        }
                    }
                }
                StmtKind::For(var, iter, body) => {
                    let items = match iter {
                        Expr::Range(..) => self.range_items(iter, env)?,
                        other => match self.eval(other, env)? {
                            Value::List(items) => items,
                            v => return self.err(format!("cannot iterate over {}", v.kind())),
                        },
                    };
                    for item in items {
                        env.scopes.push(HashMap::from([(var.clone(), item)]));
                        let flow = self.exec_stmts(body, env);
                        env.scopes.pop();
                        if let Flow::Return(v) = flow? {
                            return Ok(Flow::Return(v));
                        }
                    }
                }
                StmtKind::Return(e) => {
                    let v = match e {
                        Some(e) => self.eval(e, env)?,
                        None => Value::Unit,
                    };
                    return Ok(Flow::Return(v));
                }
                StmtKind::Listen => {}
                StmtKind::Unsupported(what) => return self.err(format!("not simulated: {what}")),
            }
        }
        Ok(Flow::Normal)
    }

    // ---- setup ---------------------------------------------------------

    fn from_json(&mut self, v: &serde_json::Value, env: &mut Env) -> R<Value> {
        use serde_json::Value as J;
        Ok(match v {
            J::Number(n) => Value::Num(n.as_f64().unwrap_or(0.0)),
            J::Bool(b) => Value::Bool(*b),
            J::Array(items) => {
                let mut out = vec![];
                for i in items {
                    out.push(self.from_json(i, env)?);
                }
                Value::List(out)
            }
            J::String(s) => {
                // "5 min" is an expression; a bare word such as AAPL is just a symbol.
                let parsed = SmlParser::parse(Rule::expr, s).ok().and_then(|mut p| p.next()).filter(|p| p.as_str().trim() == s.trim());
                match parsed {
                    Some(pair) => match self.eval(&ast::expr(pair), env) {
                        Ok(v) => v,
                        Err(_) => Value::Str(s.trim().to_owned()),
                    },
                    None => Value::Str(s.clone()),
                }
            }
            other => return self.err(format!("unsupported parameter value {other}")),
        })
    }

    fn init_machine(&mut self, overrides: &BTreeMap<String, serde_json::Value>) -> R<()> {
        let machine = self.machine;
        let mut env = Env { scopes: vec![HashMap::new()] };
        let symbols: Vec<Value> = self.market.symbols().into_iter().map(Value::Str).collect();
        for key in overrides.keys() {
            if !machine.params.iter().any(|p| &p.name == key) {
                self.st.warnings.push(format!("`{key}` is not a parameter of machine {} and was ignored", machine.name));
            }
        }
        for p in &machine.params {
            self.st.line = p.line;
            let v = if let Some(o) = overrides.get(&p.name) {
                self.from_json(o, &mut env)?
            } else {
                match (&p.default, p.ty.as_str()) {
                    (Some(Expr::List(items)), t) if items.is_empty() && t.starts_with("Vec") => Value::List(symbols.clone()),
                    (Some(d), _) => self.eval(d, &mut env)?,
                    (None, t) if t.starts_with("Vec") => Value::List(symbols.clone()),
                    (None, "Symbol") => symbols.first().cloned().unwrap_or(Value::Unit),
                    (None, "Account") => Value::Account,
                    (None, _) => return self.err(format!("parameter `{}` has no default; give it a value", p.name)),
                }
            };
            self.st.mvars.insert(p.name.clone(), v.clone());
            env.scopes[0].insert(p.name.clone(), v);
        }
        // Machine-level `let`/`static`/`const` live next to the parameters.
        let mut scope_env = Env { scopes: vec![HashMap::new()] };
        for s in &machine.vars {
            if let StmtKind::Decl { name, ty, init, .. } = &s.kind {
                self.st.line = s.line;
                let v = match init {
                    Some(e) => self.eval(e, &mut scope_env)?,
                    None => self.default_for(ty.as_deref()),
                };
                self.st.mvars.insert(name.clone(), v);
            }
        }
        Ok(())
    }

    fn launch_all(&mut self) -> R<Vec<Instance>> {
        let machine = self.machine;
        let mut out = vec![];
        for l in &machine.launches {
            self.st.line = l.line;
            let Some((proc_idx, process)) = machine.processes.iter().enumerate().find(|(_, p)| p.name == l.process) else {
                return self.err(format!("`launch` refers to process `{}`, which does not exist", l.process));
            };
            let Some(state) = process.states.iter().position(|s| s.name == l.state) else {
                return self.err(format!("`launch` starts `{}` in state `{}`, which is not declared", process.name, l.state));
            };
            if l.args.len() != process.args.len() {
                return self.err(format!("process `{}` takes {} argument(s) but `launch` passes {}", process.name, process.args.len(), l.args.len()));
            }
            // Expand `for x in xs`.
            let mut iterations: Vec<Option<(String, Value)>> = vec![None];
            if let Some((var, iter)) = &l.for_each {
                let mut env = Env { scopes: vec![HashMap::new()] };
                let items = match iter {
                    Expr::Range(..) => self.range_items(iter, &mut env)?,
                    other => match self.eval(other, &mut env)? {
                        Value::List(items) => items,
                        v => return self.err(format!("cannot iterate over {}", v.kind())),
                    },
                };
                iterations = items.into_iter().map(|v| Some((var.clone(), v))).collect();
            }
            for it in iterations {
                let mut env = Env { scopes: vec![HashMap::new()] };
                if let Some((var, v)) = &it {
                    env.scopes[0].insert(var.clone(), v.clone());
                }
                let mut args = vec![];
                for a in &l.args {
                    args.push(self.eval(a, &mut env)?);
                }
                let mut inst_env = Env { scopes: vec![HashMap::new()] };
                for ((name, _), v) in process.args.iter().zip(&args) {
                    inst_env.scopes[0].insert(name.clone(), v.clone());
                }
                let name = if args.is_empty() { process.name.clone() } else { format!("{}({})", process.name, args.iter().map(Value::show).collect::<Vec<_>>().join(", ")) };
                let every = self.every_bars(process, &mut inst_env)?;
                out.push(Instance { name, proc_idx, state, env: inst_env, started: false, every, wakes_every_bar: process.listens });
            }
        }
        Ok(out)
    }

    fn every_bars(&mut self, p: &Process, env: &mut Env) -> R<usize> {
        let secs = match &p.interval {
            Interval::Continuous => return Ok(1),
            Interval::Every(e) => {
                self.st.line = p.line;
                let v = self.eval(e, env)?;
                self.num(&v)?
            }
        };
        if !(secs.is_finite() && secs > 0.0) {
            return self.err(format!("process `{}` has an invalid interval", p.name));
        }
        Ok(((secs / self.market.bar_secs).round() as usize).max(1))
    }

    fn wake(&mut self, inst: &mut Instance) -> R<()> {
        let process = &self.machine.processes[inst.proc_idx];
        self.st.cur_process = inst.name.clone();
        if !inst.started {
            inst.started = true;
            self.exec_stmts(&process.prologue, &mut inst.env)?;
        }
        let state = &process.states[inst.state];
        self.st.cur_state = state.name.clone();
        self.st.line = state.line;
        if !state.actions.is_empty() {
            self.exec_block(&state.actions, &mut inst.env)?;
        }
        for g in &state.gotos {
            let fire = match &g.guard {
                None => true,
                Some(e) => {
                    self.st.line = g.line;
                    let v = self.eval(e, &mut inst.env)?;
                    self.truthy(&v)?
                }
            };
            if fire {
                match process.states.iter().position(|s| s.name == g.target) {
                    Some(i) => inst.state = i,
                    None => {
                        self.st.line = g.line;
                        return self.err(format!("`goto {}`: process `{}` has no state with that name", g.target, process.name));
                    }
                }
                break;
            }
        }
        Ok(())
    }
}

/// Everything recorded bar by bar.
#[derive(Debug, Clone, Default)]
pub struct Recording {
    pub equity: Vec<f64>,
    /// Drawdown from the running peak, as a non-positive percentage.
    pub drawdown_pct: Vec<f64>,
    /// Shares of the primary symbol held after each bar.
    pub position: Vec<f64>,
    /// State index of each process instance when it woke on each bar.
    pub series: Vec<Vec<usize>>,
    peak: f64,
    max_dd: f64,
    invested_bars: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Transition {
    pub bar: usize,
    pub process: String,
    pub from: String,
    pub to: String,
}

/// What one `Engine::step` did.
#[derive(Debug, Clone, Serialize)]
pub struct StepReport {
    pub bar: usize,
    pub label: String,
    pub equity: f64,
    /// Current drawdown from the peak, as a positive percentage.
    pub drawdown_pct: f64,
    pub trades: Vec<Trade>,
    pub transitions: Vec<Transition>,
    pub logs: Vec<String>,
    /// Warnings (e.g. cash ran out) and risk-limit actions.
    pub notes: Vec<String>,
    /// Orders sent to the broker during this step.
    pub orders: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PositionInfo {
    pub symbol: String,
    pub shares: f64,
    pub avg_cost: f64,
    pub last_price: f64,
    pub value: f64,
    pub unrealized_pnl: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProcessInfo {
    pub name: String,
    pub state: String,
    pub states: Vec<String>,
    pub wake_every_bars: usize,
    pub wakes_every_bar: bool,
}

/// An incremental run of a strategy: feed it one bar at a time. A backtest is
/// this over a historical series; a live deployment is this over a feed.
pub struct Engine {
    strategy: Arc<Strategy>,
    machine_idx: usize,
    market: Market,
    st: State,
    instances: Vec<Instance>,
    rec: Recording,
    primary: String,
    initial_cash: f64,
}

fn plain(message: impl Into<String>) -> SimError {
    SimError { line: None, bar: None, message: message.into() }
}

fn fail_to_err(f: Fail, bar: usize) -> SimError {
    match f {
        Fail::Err(e) => e,
        Fail::NoHistory => SimError { line: None, bar: Some(bar), message: "not enough price history while setting up the strategy".into() },
    }
}

impl Engine {
    /// Sets the strategy up (parameters, launches) without consuming any bars yet.
    pub fn new(strategy: Arc<Strategy>, symbols: &[String], bar_secs: f64, cfg: &Config) -> Result<Engine, SimError> {
        let broker = bk::PaperBroker::new(cfg.initial_cash, cfg.fee_bps);
        Self::with_broker(strategy, symbols, bar_secs, cfg, Box::new(broker))
    }

    /// Like `new`, but orders go to `broker` (a simulated account, or a real brokerage).
    pub fn with_broker(strategy: Arc<Strategy>, symbols: &[String], bar_secs: f64, cfg: &Config, broker: Box<dyn Broker>) -> Result<Engine, SimError> {
        if symbols.is_empty() || symbols.len() > market::MAX_SYMBOLS {
            return Err(plain(format!("between 1 and {} symbols are required", market::MAX_SYMBOLS)));
        }
        if !(bar_secs.is_finite() && bar_secs >= 1.0) {
            return Err(plain("bar_seconds must be at least 1"));
        }
        if !(cfg.initial_cash.is_finite() && cfg.initial_cash > 0.0) {
            return Err(plain("initial_cash must be positive"));
        }
        if !(cfg.fee_bps.is_finite() && (0.0..=1000.0).contains(&cfg.fee_bps)) {
            return Err(plain("fee_bps must be between 0 and 1000"));
        }
        if let Some(cap) = cfg.risk.max_position_value {
            if !(cap.is_finite() && cap > 0.0) {
                return Err(plain("max_position_value must be positive"));
            }
        }
        let machine_idx = match &cfg.machine {
            Some(n) => strategy.machines.iter().position(|m| &m.name == n).ok_or_else(|| plain(format!("there is no machine named `{n}`")))?,
            None if strategy.machines.is_empty() => return Err(plain("the strategy defines no machine")),
            None => 0,
        };
        let market = Market { bar_secs, series: symbols.iter().map(|s| (s.clone(), vec![])).collect() };
        let (st, instances) = {
            let machine = &strategy.machines[machine_idx];
            let events: HashMap<&str, &EventDef> = strategy.events.iter().chain(&machine.events).map(|e| (e.name.as_str(), e)).collect();
            let simulated = broker.environment() == bk::Environment::Simulated;
            let prefix = cfg.client_prefix.clone().unwrap_or_else(|| "sim".into());
            let st = State { fee: if simulated { cfg.fee_bps / 10_000.0 } else { 0.0 }, risk: cfg.risk.clone(), broker: Some(broker), prefix, ..Default::default() };
            let mut it = Interp { machine, market: &market, events, st };
            it.init_machine(&cfg.params).map_err(|f| fail_to_err(f, 0))?;
            let instances = it.launch_all().map_err(|f| fail_to_err(f, 0))?;
            (it.st, instances)
        };
        if instances.is_empty() {
            return Err(plain("no process was launched: add `init { launch (process, STATE); }` to the machine"));
        }
        let rec = Recording { series: vec![vec![]; instances.len()], peak: f64::MIN, ..Default::default() };
        let primary = market.symbols()[0].clone();
        Ok(Engine { strategy, machine_idx, market, st, instances, rec, primary, initial_cash: cfg.initial_cash })
    }

    fn machine(&self) -> &Machine {
        &self.strategy.machines[self.machine_idx]
    }

    pub fn machine_name(&self) -> &str {
        &self.machine().name
    }

    pub fn primary(&self) -> &str {
        &self.primary
    }

    pub fn bar_count(&self) -> usize {
        self.market.len()
    }

    pub fn bar_secs(&self) -> f64 {
        self.market.bar_secs
    }

    pub fn bars(&self, symbol: &str) -> &[Bar] {
        self.market.series.get(symbol).map_or(&[], |v| v.as_slice())
    }

    pub fn recording(&self) -> &Recording {
        &self.rec
    }

    pub fn trades(&self) -> &[Trade] {
        &self.st.trades
    }

    pub fn logs(&self) -> &[String] {
        &self.st.logs
    }

    pub fn warnings(&self) -> &[String] {
        &self.st.warnings
    }

    /// Feeds the next bar of every symbol and runs one wake-up cycle.
    pub fn step(&mut self, bars: &BTreeMap<String, Bar>) -> Result<StepReport, SimError> {
        for sym in self.market.series.keys() {
            match bars.get(sym) {
                None => return Err(plain(format!("the feed delivered no bar for {sym}"))),
                Some(b) if !(b.close.is_finite() && b.close > 0.0) => return Err(plain(format!("{sym}: invalid close price {}", b.close))),
                _ => {}
            }
        }
        for (sym, series) in self.market.series.iter_mut() {
            series.push(bars[sym].clone());
        }
        let idx = self.market.len() - 1;
        let strategy = self.strategy.clone();
        let machine = &strategy.machines[self.machine_idx];
        let events: HashMap<&str, &EventDef> = strategy.events.iter().chain(&machine.events).map(|e| (e.name.as_str(), e)).collect();
        let mut st = std::mem::take(&mut self.st);
        st.bar = idx;
        st.steps = 0; // the runaway-loop guard applies per bar, not over the lifetime of a deployment
        let (trades_before, logs_before, warn_before) = (st.trades.len(), st.logs.len(), st.warnings.len());
        let mut it = Interp { machine, market: &self.market, events, st };

        let mut transitions = vec![];
        let mut failure = None;
        let candles: BTreeMap<String, bk::Candle> =
            bars.iter().map(|(s, b)| (s.clone(), bk::Candle { open: b.open, high: b.high, low: b.low, close: b.close })).collect();
        if let Err(f) = it.begin_bar(&candles) {
            failure = Some(fail_to_err(f, idx));
        }
        for (k, inst) in self.instances.iter_mut().enumerate() {
            if failure.is_some() {
                break;
            }
            self.rec.series[k].push(inst.state);
            if inst.wakes_every_bar || idx % inst.every == 0 {
                let before = inst.state;
                match it.wake(inst) {
                    Ok(()) | Err(Fail::NoHistory) => {}
                    Err(f) => {
                        failure = Some(fail_to_err(f, idx));
                        break;
                    }
                }
                if inst.state != before {
                    let states = &machine.processes[inst.proc_idx].states;
                    transitions.push(Transition { bar: idx, process: inst.name.clone(), from: states[before].name.clone(), to: states[inst.state].name.clone() });
                }
            }
        }
        let equity = match it.equity() {
            Ok(e) => e,
            Err(f) => {
                failure.get_or_insert(fail_to_err(f, idx));
                0.0
            }
        };
        let mut st = it.st;
        if let Some(b) = st.broker.as_mut() {
            let extra = b.drain_notes();
            st.notes.extend(extra);
        }
        if let Some(e) = failure {
            self.st = st;
            return Err(e);
        }
        self.record(&st, idx, equity);
        let report = StepReport {
            bar: idx,
            label: self.market.series[&self.primary][idx].label.clone(),
            equity,
            drawdown_pct: -self.rec.drawdown_pct[idx],
            trades: st.trades[trades_before..].to_vec(),
            transitions,
            logs: st.logs[logs_before..].to_vec(),
            notes: st.warnings[warn_before..].iter().cloned().chain(std::mem::take(&mut st.notes)).collect(),
            orders: std::mem::take(&mut st.placed),
        };
        self.st = st;
        Ok(report)
    }

    fn record(&mut self, st: &State, idx: usize, equity: f64) {
        let rec = &mut self.rec;
        rec.peak = rec.peak.max(equity);
        let dd = if rec.peak > 0.0 { (rec.peak - equity) / rec.peak * 100.0 } else { 0.0 };
        rec.max_dd = rec.max_dd.max(dd);
        rec.equity.push(equity);
        rec.drawdown_pct.push(-dd);
        rec.position.push(st.holdings.get(&self.primary).map_or(0.0, |h| h.shares));
        if st.holdings.values().any(|h| h.shares > 0.0) {
            rec.invested_bars += 1;
        }
        debug_assert_eq!(rec.equity.len(), idx + 1);
    }

    /// Sells every position at the latest close (a manual intervention).
    pub fn flatten(&mut self) -> Result<Vec<Trade>, SimError> {
        if self.market.is_empty() {
            return Ok(vec![]);
        }
        let idx = self.market.len() - 1;
        let strategy = self.strategy.clone();
        let machine = &strategy.machines[self.machine_idx];
        let events: HashMap<&str, &EventDef> = HashMap::new();
        let mut st = std::mem::take(&mut self.st);
        let before = st.trades.len();
        st.bar = idx;
        st.cur_process = "manual".into();
        st.cur_state = "flatten".into();
        let mut it = Interp { machine, market: &self.market, events, st };
        let held: Vec<(String, f64)> = it.st.holdings.iter().filter(|(_, h)| h.shares > 0.0).map(|(s, h)| (s.clone(), h.shares)).collect();
        let mut failure = None;
        for (sym, shares) in held {
            if let Err(f) = it.fill(false, &sym, shares) {
                failure = Some(fail_to_err(f, idx));
                break;
            }
        }
        let equity = match it.equity() {
            Ok(e) => e,
            Err(f) => {
                failure.get_or_insert(fail_to_err(f, idx));
                self.rec.equity.last().copied().unwrap_or(self.initial_cash)
            }
        };
        let st = it.st;
        let mut st = st;
        st.placed.clear();
        let trades = st.trades[before..].to_vec();
        // Replace the last recorded point: the liquidation happened on this bar.
        self.rec.equity.pop();
        self.rec.drawdown_pct.pop();
        self.rec.position.pop();
        let invested = self.rec.invested_bars;
        if st.holdings.values().all(|h| h.shares == 0.0) && invested > 0 {
            self.rec.invested_bars -= 1;
        }
        self.record(&st, idx, equity);
        self.st = st;
        match failure {
            Some(e) => Err(e),
            None => Ok(trades),
        }
    }

    pub fn stats(&self) -> Stats {
        let n = self.market.len().max(1);
        let primary = &self.market.series[&self.primary];
        let equity = self.rec.equity.last().copied().unwrap_or(self.initial_cash);
        let buy_hold_final = match (primary.first(), primary.last()) {
            (Some(first), Some(last)) => self.initial_cash / first.close * last.close,
            _ => self.initial_cash,
        };
        Stats {
            bars: self.market.len(),
            initial_cash: self.initial_cash,
            final_equity: equity,
            total_return_pct: (equity / self.initial_cash - 1.0) * 100.0,
            buy_hold_return_pct: (buy_hold_final / self.initial_cash - 1.0) * 100.0,
            max_drawdown_pct: self.rec.max_dd,
            fills: self.st.trades.len(),
            closed_trades: self.st.closes,
            win_rate_pct: (self.st.closes > 0).then(|| self.st.wins as f64 / self.st.closes as f64 * 100.0),
            realized_pnl: self.st.realized,
            exposure_pct: self.rec.invested_bars as f64 / n as f64 * 100.0,
        }
    }

    /// Cash as of the last bar (the broker's account).
    pub fn cash(&self) -> f64 {
        self.st.account.as_ref().map_or(self.initial_cash, |a| a.cash)
    }

    pub fn max_position_value(&self) -> Option<f64> {
        self.st.risk.max_position_value
    }

    /// Changes the position-size guard of a running engine.
    pub fn set_max_position_value(&mut self, cap: Option<f64>) -> Result<(), SimError> {
        if let Some(c) = cap {
            if !(c.is_finite() && c > 0.0) {
                return Err(plain("max_position_value must be positive"));
            }
        }
        self.st.risk.max_position_value = cap;
        Ok(())
    }

    /// The most recent orders, newest first, as last seen.
    pub fn recent_orders(&self, n: usize) -> Vec<bk::Order> {
        self.st.recent.iter().rev().filter_map(|id| self.st.orders.get(id)).take(n).cloned().collect()
    }

    pub fn working_orders(&self) -> usize {
        self.st.working.len()
    }

    pub fn broker_environment(&self) -> bk::Environment {
        self.st.broker.as_ref().map_or(bk::Environment::Simulated, |b| b.environment())
    }

    pub fn account(&self) -> Option<&bk::Account> {
        self.st.account.as_ref()
    }

    pub fn broker_name(&self) -> String {
        self.st.broker.as_ref().map(|b| b.name().to_owned()).unwrap_or_default()
    }

    /// Cancels every working order at the broker (positions are left alone).
    /// Returns the notes it produced and how many orders are still working.
    pub fn cancel_all_orders(&mut self) -> Result<(usize, Vec<String>), SimError> {
        let idx = self.market.len().saturating_sub(1);
        let strategy = self.strategy.clone();
        let machine = &strategy.machines[self.machine_idx];
        let events: HashMap<&str, &EventDef> = HashMap::new();
        let mut st = std::mem::take(&mut self.st);
        st.bar = idx;
        let before = st.working.len();
        let mut it = Interp { machine, market: &self.market, events, st };
        let mut failure = None;
        for id in it.st.working.clone() {
            if let Err(f) = it.cancel(&id) {
                failure = Some(fail_to_err(f, idx));
                break;
            }
        }
        let mut st = it.st;
        let notes = std::mem::take(&mut st.notes);
        let left = st.working.len();
        self.st = st;
        match failure {
            Some(e) => Err(e),
            None => Ok((before - left, notes)),
        }
    }

    pub fn positions(&self) -> Vec<PositionInfo> {
        let last = self.market.len().saturating_sub(1);
        self.st
            .holdings
            .iter()
            .filter(|(_, h)| h.shares > 0.0)
            .map(|(sym, h)| {
                let price = self.market.series[sym].get(last).map_or(0.0, |b| b.close);
                PositionInfo { symbol: sym.clone(), shares: h.shares, avg_cost: h.avg_cost, last_price: price, value: h.shares * price, unrealized_pnl: (price - h.avg_cost) * h.shares }
            })
            .collect()
    }

    pub fn processes(&self) -> Vec<ProcessInfo> {
        let machine = self.machine();
        self.instances
            .iter()
            .map(|i| {
                let p = &machine.processes[i.proc_idx];
                ProcessInfo {
                    name: i.name.clone(),
                    state: p.states[i.state].name.clone(),
                    states: p.states.iter().map(|s| s.name.clone()).collect(),
                    wake_every_bars: i.every,
                    wakes_every_bar: i.wakes_every_bar,
                }
            })
            .collect()
    }

    pub fn final_states(&self) -> BTreeMap<String, String> {
        self.processes().into_iter().map(|p| (p.name, p.state)).collect()
    }

    pub fn timelines(&self) -> Vec<Timeline> {
        self.timelines_from(0)
    }

    /// The state timelines from bar `from` on (cloning only that slice).
    pub fn timelines_from(&self, from: usize) -> Vec<Timeline> {
        let machine = self.machine();
        self.instances
            .iter()
            .zip(&self.rec.series)
            .map(|(inst, ser)| Timeline {
                process: inst.name.clone(),
                states: machine.processes[inst.proc_idx].states.iter().map(|s| s.name.clone()).collect(),
                series: ser[from.min(ser.len())..].to_vec(),
            })
            .collect()
    }

    pub fn into_backtest(self) -> Backtest {
        let stats = self.stats();
        let primary_bars = self.market.series[&self.primary].clone();
        let first_close = primary_bars.first().map_or(1.0, |b| b.close);
        let buy_hold = primary_bars.iter().map(|b| self.initial_cash / first_close * b.close).collect();
        Backtest {
            machine: self.machine().name.clone(),
            symbol: self.primary.clone(),
            bars: primary_bars,
            equity: self.rec.equity.clone(),
            buy_hold,
            drawdown_pct: self.rec.drawdown_pct.clone(),
            position: self.rec.position.clone(),
            timelines: self.timelines(),
            final_states: self.final_states(),
            trades: self.st.trades,
            stats,
            warnings: self.st.warnings,
            logs: self.st.logs,
        }
    }
}

/// Runs the strategy over the market and returns the full backtest record.
pub fn backtest(strat: &Strategy, market: &Market, cfg: &Config) -> Result<Backtest, SimError> {
    market.validate().map_err(plain)?;
    let mut engine = Engine::new(Arc::new(strat.clone()), &market.symbols(), market.bar_secs, cfg)?;
    for i in 0..market.len() {
        let bars: BTreeMap<String, Bar> = market.series.iter().map(|(s, v)| (s.clone(), v[i].clone())).collect();
        engine.step(&bars)?;
    }
    Ok(engine.into_backtest())
}

/// Convenience for tests and the CLI: close prices for a single symbol.
pub fn market_from_closes(symbol: &str, closes: &[f64], bar_secs: f64) -> Market {
    Market::single(symbol, market::bars_from_closes(closes, bar_secs), bar_secs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::parse;

    const THRESHOLD: &str = include_str!("../../../examples/threshold.sm");

    fn run(src: &str, closes: &[f64], cfg: Config) -> Result<Backtest, SimError> {
        backtest(&parse(src).unwrap(), &market_from_closes("TEST", closes, 86_400.0), &cfg)
    }

    fn cfg(params: serde_json::Value) -> Config {
        Config { params: serde_json::from_value(params).unwrap(), ..Default::default() }
    }

    #[test]
    fn threshold_trades_with_one_cycle_delay_per_transition() {
        let b = run(THRESHOLD, &[12.0, 11.0, 9.0, 9.5, 16.0, 17.0, 17.0], cfg(serde_json::json!({"floor": 10, "ceiling": 15}))).unwrap();
        // bar 2: WAIT sees 9 < 10 -> BUY; bar 3: BUY runs (fills at 9.5) -> HOLD;
        // bar 4: HOLD sees 16 > 15 -> SELL; bar 5: SELL runs (fills at 17) -> WAIT.
        let seen: Vec<_> = b.trades.iter().map(|t| (t.bar, t.side, t.price)).collect();
        assert_eq!(seen, [(3, "buy", 9.5), (5, "sell", 17.0)]);
        let shares = 500.0 / 9.5;
        assert!((b.trades[0].shares - shares).abs() < 1e-9);
        assert!((b.trades[1].pnl.unwrap() - (17.0 - 9.5) * shares).abs() < 1e-6);
        assert_eq!(b.final_states["watch(TEST)"], "WAIT");
        assert_eq!(b.position, [0.0, 0.0, 0.0, shares, shares, 0.0, 0.0]);
        assert!((b.stats.final_equity - (100_000.0 + (17.0 - 9.5) * shares)).abs() < 1e-6);
        assert_eq!((b.stats.fills, b.stats.closed_trades), (2, 1));
        assert_eq!(b.stats.win_rate_pct, Some(100.0));
    }

    #[test]
    fn timeline_records_the_state_each_bar_woke_in() {
        let b = run(THRESHOLD, &[12.0, 11.0, 9.0, 9.5, 16.0, 17.0], cfg(serde_json::json!({"floor": 10, "ceiling": 15}))).unwrap();
        let t = &b.timelines[0];
        let names: Vec<_> = t.series.iter().map(|&i| t.states[i].as_str()).collect();
        assert_eq!(names, ["WAIT", "WAIT", "WAIT", "BUY", "HOLD", "SELL"]);
    }

    #[test]
    fn no_trades_when_the_price_never_crosses() {
        let b = run(THRESHOLD, &[12.0, 13.0, 14.0], cfg(serde_json::json!({"floor": 10, "ceiling": 15}))).unwrap();
        assert!(b.trades.is_empty());
        assert_eq!(b.stats.total_return_pct, 0.0);
        assert!((b.stats.buy_hold_return_pct - (14.0 / 12.0 - 1.0) * 100.0).abs() < 1e-9);
    }

    #[test]
    fn fees_and_cash_limits() {
        let mut c = cfg(serde_json::json!({"floor": 10, "ceiling": 15, "invest": 1000}));
        c.initial_cash = 100.0;
        let b = run(THRESHOLD, &[9.0, 9.0, 9.0, 20.0, 20.0, 20.0], c).unwrap();
        assert!(b.warnings.iter().any(|w| w.contains("not enough cash")), "{:?}", b.warnings);
        assert!((b.trades[0].shares - 100.0 / 9.0).abs() < 1e-9);

        let mut c = cfg(serde_json::json!({"floor": 10, "ceiling": 15, "invest": 900}));
        c.fee_bps = 100.0; // 1%
        let b = run(THRESHOLD, &[9.0, 9.0, 9.0, 20.0, 20.0, 20.0], c).unwrap();
        let t = &b.trades;
        assert_eq!(t.len(), 2);
        let fee_in = t[0].shares * 9.0 * 0.01;
        let fee_out = t[1].shares * 20.0 * 0.01;
        assert!((b.stats.final_equity - (100_000.0 + (20.0 - 9.0) * t[0].shares - fee_in - fee_out)).abs() < 1e-6);
    }

    #[test]
    fn unknown_override_is_a_warning_and_missing_default_is_an_error() {
        let b = run(THRESHOLD, &[12.0, 13.0], cfg(serde_json::json!({"nope": 1}))).unwrap();
        assert!(b.warnings.iter().any(|w| w.contains("`nope`")));
        let e = run("machine M { param x: float; init { launch (p, A); } process p { state A; } }", &[1.0], Config::default()).unwrap_err();
        assert!(e.message.contains("parameter `x` has no default"), "{e}");
    }

    #[test]
    fn events_history_indexing_and_statics() {
        let src = r#"
event rising(const signal ticker: Ticker) {
    for i in 0..2 {
        if ticker[-i days] < ticker[-(i+1) days] { return false; }
    }
    return true;
}
machine M {
    param sym: Symbol;
    param n: float = 1.0;
    init { launch (p, WAIT); }
    process p every 1 day {
        let pos: Position;
        listen Tickers[sym];
        state WAIT: goto BUY on rising(Tickers[sym]);
        state BUY: goto HOLD { pos = account.buy(sym, n); }
        state HOLD;
    }
}
"#;
        // Needs two prior bars; strictly rising 10, 11, 12 -> bar 2 sees rising, bar 3 buys.
        let b = run(src, &[10.0, 11.0, 12.0, 13.0, 14.0], Config::default()).unwrap_err();
        // `account` is not a parameter here: the error must say so and name the line.
        assert!(b.message.contains("unknown name `account`"), "{b}");
        assert!(b.line.is_some());

        let src = src.replace("param n: float = 1.0;", "param n: float = 1.0;\n    param account: Account = SandBox;");
        let b = run(&src, &[10.0, 11.0, 12.0, 13.0, 14.0], Config::default()).unwrap();
        assert_eq!(b.trades.len(), 1);
        assert_eq!((b.trades[0].bar, b.trades[0].price), (3, 13.0));
        // Not enough history on bars 0 and 1: the process simply stays in WAIT.
        let t = &b.timelines[0];
        assert_eq!(t.series[..3], [0, 0, 0]);
    }

    #[test]
    fn falling_prices_do_not_trigger_the_rising_event() {
        let src = r#"
event rising(const signal ticker: Ticker) {
    for i in 0..2 { if ticker[-i days] < ticker[-(i+1) days] { return false; } }
    return true;
}
machine M {
    param sym: Symbol;
    param account: Account = SandBox;
    init { launch (p, WAIT); }
    process p { listen Tickers[sym]; state WAIT: goto BUY on rising(Tickers[sym]); state BUY { account.buy(sym, 1); } }
}
"#;
        let b = run(src, &[10.0, 9.0, 8.0, 7.0], Config::default()).unwrap();
        assert!(b.trades.is_empty());
    }

    #[test]
    fn unsupported_constructs_stop_the_run_with_a_line() {
        let src = "machine M {\n param sym: Symbol;\n init { launch (p, A); }\n process p {\n  state A: goto A { x = Tickers[sym].history(2 days); }\n }\n}";
        let e = run(src, &[1.0, 2.0], Config::default()).unwrap_err();
        assert!(e.message.contains("history"), "{e}");
        assert_eq!(e.line, Some(5));

        let e = run("machine M { init { launch (p, A); } process p { state A: goto A { record x in y; } } }", &[1.0], Config::default()).unwrap_err();
        assert!(e.message.contains("not simulated"), "{e}");
    }

    #[test]
    fn actions_repeat_every_wake_up_until_a_guard_fires() {
        // The pitfall documented in docs/sml-language.md: a guarded state keeps running its action.
        let src = "machine M { param sym: Symbol; param account: Account = SandBox; init { launch (p, A); }
            process p { listen Tickers[sym]; state A: goto B on Tickers[sym] > 100 { account.buy(sym, 1); } state B; } }";
        let b = run(src, &[10.0, 10.0, 10.0], Config::default()).unwrap();
        assert_eq!(b.trades.len(), 3);
    }

    #[test]
    fn process_interval_maps_to_bars() {
        let src = "machine M { param sym: Symbol; param account: Account = SandBox; init { launch (p, A); }
            process p every 3 days { state A: goto A { account.buy(sym, 1); } } }";
        let b = run(src, &[1.0; 10], Config::default()).unwrap();
        let bars: Vec<_> = b.trades.iter().map(|t| t.bar).collect();
        assert_eq!(bars, [0, 3, 6, 9]);
    }

    #[test]
    fn runaway_loops_are_stopped() {
        let src = "machine M { init { launch (p, A); } process p { state A: goto A { loop { } } } }";
        let e = run(src, &[1.0], Config::default()).unwrap_err();
        assert!(e.message.contains("iterations"), "{e}");
    }

    #[test]
    fn momentum_example_enters_after_two_up_days_and_exits_on_target_or_stop() {
        let src = include_str!("../../../examples/momentum.sm");
        // Up, up (enter next bar at 12), +10% target is 13.2 -> exit when price >= 13.2.
        let closes = [10.0, 11.0, 11.5, 12.0, 12.5, 13.5, 13.0, 12.0, 11.0, 10.0];
        let b = run(src, &closes, Config::default()).unwrap();
        let seen: Vec<_> = b.trades.iter().map(|t| (t.bar, t.side, t.price)).collect();
        // bar 2 sees 11.5 >= 11 >= 10 -> ENTER; bar 3 buys at 12; bar 4/5: 13.5 >= 13.2 -> PROFIT; bar 6 sells at 13.
        assert_eq!(seen[0], (3, "buy", 12.0));
        assert_eq!(seen[1], (6, "sell", 13.0));
        // After the exit the process is FLAT and re-enters once two up-days are seen again (not here: prices fall).
        assert_eq!(b.final_states["trade(TEST)"], "FLAT");
    }

    fn engine(src: &str, cfg: &Config) -> Engine {
        Engine::new(Arc::new(parse(src).unwrap()), &["TEST".to_string()], 86_400.0, cfg).unwrap()
    }

    fn bar_of(price: f64, i: usize) -> BTreeMap<String, Bar> {
        let mut b = market::bars_from_closes(&[price], 86_400.0).remove(0);
        b.label = market::label(i, 86_400.0);
        BTreeMap::from([("TEST".to_string(), b)])
    }

    #[test]
    fn stepping_bar_by_bar_gives_the_same_result_as_a_batch_backtest() {
        let closes = [12.0, 11.0, 9.0, 9.5, 16.0, 17.0, 17.0, 8.0, 9.0, 16.0, 16.0];
        let c = cfg(serde_json::json!({"floor": 10, "ceiling": 15}));
        let batch = run(THRESHOLD, &closes, c.clone()).unwrap();
        let mut e = engine(THRESHOLD, &c);
        let mut seen_trades = vec![];
        for (i, p) in closes.iter().enumerate() {
            let r = e.step(&bar_of(*p, i)).unwrap();
            assert_eq!(r.bar, i);
            seen_trades.extend(r.trades);
        }
        assert_eq!(seen_trades, batch.trades);
        assert_eq!(e.recording().equity, batch.equity);
        assert_eq!(e.final_states(), batch.final_states);
        let b2 = e.into_backtest();
        assert_eq!(b2.stats.fills, batch.stats.fills);
        assert_eq!(b2.timelines[0].series, batch.timelines[0].series);
    }

    #[test]
    fn step_reports_trades_and_state_transitions_as_they_happen() {
        let mut e = engine(THRESHOLD, &cfg(serde_json::json!({"floor": 10, "ceiling": 15})));
        let mut transitions = vec![];
        let mut fills = vec![];
        for (i, p) in [12.0, 9.0, 9.5, 16.0, 17.0].iter().enumerate() {
            let r = e.step(&bar_of(*p, i)).unwrap();
            transitions.extend(r.transitions.iter().map(|t| format!("{}:{}->{}", t.bar, t.from, t.to)));
            fills.extend(r.trades.iter().map(|t| format!("{}:{}", t.bar, t.side)));
        }
        assert_eq!(transitions, ["1:WAIT->BUY", "2:BUY->HOLD", "3:HOLD->SELL", "4:SELL->WAIT"]);
        assert_eq!(fills, ["2:buy", "4:sell"]);
        assert_eq!(e.processes()[0].state, "WAIT");
    }

    #[test]
    fn positions_and_cash_are_visible_while_running() {
        let mut e = engine(THRESHOLD, &cfg(serde_json::json!({"floor": 10, "ceiling": 15})));
        for (i, p) in [9.0, 9.0, 12.0].iter().enumerate() {
            e.step(&bar_of(*p, i)).unwrap();
        }
        let pos = e.positions();
        assert_eq!(pos.len(), 1);
        assert!((pos[0].shares - 500.0 / 9.0).abs() < 1e-9);
        assert_eq!(pos[0].last_price, 12.0);
        assert!((pos[0].unrealized_pnl - (12.0 - 9.0) * 500.0 / 9.0).abs() < 1e-9);
        assert!((e.cash() - 99_500.0).abs() < 1e-9);
        assert_eq!(e.processes()[0].state, "HOLD");
    }

    #[test]
    fn flatten_sells_everything_at_the_last_price_and_updates_the_equity_point() {
        let mut e = engine(THRESHOLD, &cfg(serde_json::json!({"floor": 10, "ceiling": 15})));
        for (i, p) in [9.0, 9.0, 12.0].iter().enumerate() {
            e.step(&bar_of(*p, i)).unwrap();
        }
        let equity_before = *e.recording().equity.last().unwrap();
        let trades = e.flatten().unwrap();
        assert_eq!(trades.len(), 1);
        assert_eq!((trades[0].side, trades[0].price, trades[0].process.as_str(), trades[0].state.as_str()), ("sell", 12.0, "manual", "flatten"));
        assert!(e.positions().is_empty());
        assert!((e.cash() - (100_000.0 + (12.0 - 9.0) * 500.0 / 9.0)).abs() < 1e-6);
        // No fees: liquidating at the same price leaves equity unchanged, and the series length is intact.
        assert!((*e.recording().equity.last().unwrap() - equity_before).abs() < 1e-6);
        assert_eq!(e.recording().equity.len(), 3);
        assert_eq!(e.recording().position.len(), 3);
        assert!(e.flatten().unwrap().is_empty(), "nothing left to sell");
        // The strategy continues afterwards.
        let r = e.step(&bar_of(13.0, 3)).unwrap();
        assert_eq!(r.bar, 3);
    }

    #[test]
    fn position_value_cap_reduces_buys_and_says_so() {
        let mut c = cfg(serde_json::json!({"floor": 10, "ceiling": 15, "invest": 1000}));
        c.risk.max_position_value = Some(300.0);
        let mut e = engine(THRESHOLD, &c);
        let mut notes = vec![];
        for (i, p) in [9.0, 9.0, 9.0].iter().enumerate() {
            notes.extend(e.step(&bar_of(*p, i)).unwrap().notes);
        }
        let pos = e.positions();
        assert!((pos[0].value - 300.0).abs() < 1e-6, "{pos:?}");
        assert!(notes.iter().any(|n| n.contains("risk limit")), "{notes:?}");
        let mut bad = c.clone();
        bad.risk.max_position_value = Some(-1.0);
        assert!(Engine::new(Arc::new(parse(THRESHOLD).unwrap()), &["TEST".to_string()], 86_400.0, &bad).is_err());
    }

    #[test]
    fn a_runtime_error_is_returned_and_the_engine_stays_inspectable() {
        let src = "machine M { param sym: Symbol; init { launch (p, A); } process p { state A: goto A { record x in y; } } }";
        let mut e = engine(src, &Config::default());
        let err = e.step(&bar_of(5.0, 0)).unwrap_err();
        assert!(err.message.contains("not simulated"));
        assert_eq!(e.bar_count(), 1);
        assert_eq!(e.processes()[0].state, "A");
    }

    #[test]
    fn step_requires_a_bar_for_every_symbol_and_a_valid_price() {
        let mut e = engine(THRESHOLD, &Config::default());
        assert!(e.step(&BTreeMap::new()).unwrap_err().message.contains("no bar for TEST"));
        assert!(e.step(&bar_of(0.0, 0)).unwrap_err().message.contains("invalid close"));
        assert_eq!(e.bar_count(), 0, "a rejected bar is not recorded");
    }

    #[test]
    fn the_step_limit_is_per_bar_so_long_runs_do_not_hit_it() {
        let mut e = engine(THRESHOLD, &cfg(serde_json::json!({"floor": 10, "ceiling": 15})));
        for i in 0..300 {
            e.step(&bar_of(12.0, i)).unwrap();
        }
        // Pretend a lot of work was already done: the counter must not carry over.
        e.st.steps = MAX_STEPS - 5;
        e.step(&bar_of(12.0, 300)).unwrap();
        assert!(e.st.steps < 1000);
    }

    #[test]
    fn config_is_validated() {
        let s = parse(THRESHOLD).unwrap();
        let m = market_from_closes("T", &[1.0, 2.0], 86_400.0);
        assert!(backtest(&s, &m, &Config { initial_cash: 0.0, ..Default::default() }).is_err());
        assert!(backtest(&s, &m, &Config { fee_bps: -1.0, ..Default::default() }).is_err());
        assert!(backtest(&s, &m, &Config { machine: Some("Nope".into()), ..Default::default() }).unwrap_err().message.contains("Nope"));
    }

    #[test]
    fn printf_formats_into_the_log() {
        let src = "machine M { param sym: Symbol; init { launch (p, A); } process p { state A: goto A { printf(\"price is %f\", Tickers[sym]); } } }";
        let b = run(src, &[4.5], Config::default()).unwrap();
        assert_eq!(b.logs, ["bar 0: price is 4.5"]);
    }
}

#[cfg(test)]
mod example_tests {
    use crate::model::parse;
    use crate::testing;

    /// The shipped example tests must pass against the shipped example strategies.
    #[test]
    fn shipped_example_tests_pass() {
        for (src, tests) in [
            (include_str!("../../../examples/threshold.sm"), include_str!("../../../examples/threshold.test.json")),
            (include_str!("../../../examples/momentum.sm"), include_str!("../../../examples/momentum.test.json")),
            (include_str!("../../../examples/bracket.sm"), include_str!("../../../examples/bracket.test.json")),
        ] {
            let results = testing::run_tests(&parse(src).unwrap(), &testing::parse_tests(tests).unwrap());
            for r in &results {
                assert!(r.passed, "{}: {:?} {:?}", r.name, r.failures, r.error);
            }
        }
    }

}

#[cfg(test)]
mod order_tests {
    use super::*;
    use crate::model::parse;

    const THRESHOLD: &str = include_str!("../../../examples/threshold.sm");

    fn run(src: &str, closes: &[f64], cfg: Config) -> Result<Backtest, SimError> {
        backtest(&parse(src).unwrap(), &market_from_closes("TEST", closes, 86_400.0), &cfg)
    }

    fn cfg(params: serde_json::Value) -> Config {
        Config { params: serde_json::from_value(params).unwrap(), ..Default::default() }
    }

    fn bar_of(price: f64, i: usize) -> BTreeMap<String, Bar> {
        let mut b = market::bars_from_closes(&[price], 86_400.0).remove(0);
        b.label = market::label(i, 86_400.0);
        BTreeMap::from([("TEST".to_string(), b)])
    }

    const BRACKET: &str = include_str!("../../../examples/bracket.sm");

    fn bracket(closes: &[f64]) -> Backtest {
        run(BRACKET, closes, cfg(serde_json::json!({}))).unwrap()
    }

    #[test]
    fn a_limit_entry_rests_until_the_dip_and_the_profit_target_fills_later() {
        let b = bracket(&[100.0, 100.0, 97.0, 96.0, 100.0, 105.0, 105.0, 105.0]);
        let seen: Vec<_> = b.trades.iter().map(|t| (t.bar, t.side, (t.price * 100.0).round() / 100.0)).collect();
        assert_eq!(seen, [(2, "buy", 98.0), (5, "sell", 101.92)]);
        assert!((b.trades[0].shares - 1000.0 / 98.0).abs() < 1e-9);
        assert!(b.trades[1].pnl.unwrap() > 0.0);
        // the untriggered stop was cancelled, so nothing else happens afterwards
        assert_eq!(b.stats.fills, 2);
        assert_eq!(b.position.last().copied(), Some(0.0));
    }

    #[test]
    fn the_stop_loss_fills_at_the_stop_and_the_other_exit_is_cancelled() {
        let b = bracket(&[100.0, 100.0, 97.0, 96.0, 90.0, 90.0, 90.0, 90.0]);
        let sells: Vec<_> = b.trades.iter().filter(|t| t.side == "sell").collect();
        assert_eq!(sells.len(), 1);
        assert!((sells[0].price - 98.0 * 0.97).abs() < 1e-9, "{}", sells[0].price);
        assert!(sells[0].pnl.unwrap() < 0.0);
    }

    #[test]
    fn account_numbers_order_status_and_cancel_are_available_to_the_strategy() {
        let src = "machine M { param sym: Symbol; param account: Account = SandBox; init { launch (p, A); }
            process p { let o: Order; let c: float; let q: float; let st: string; listen Tickers[sym];
              state A: goto B { o = account.buy_limit(sym, 10, 50); c = account.cash(); q = account.last(sym); }
              state B: goto C on o.is_open() { st = o.status(); printf(\"%s %s %s\", st, c, q); }
              state C: goto D { printf(\"cancelled=%s\", o.cancel()); }
              state D { printf(\"final %s\", o.status()); } } }";
        let b = run(src, &[100.0, 100.0, 100.0, 100.0, 100.0], cfg(serde_json::json!({"sym": "TEST"}))).unwrap();
        assert!(b.trades.is_empty());
        assert!(b.logs.iter().any(|l| l.contains("open 100000 100")), "{:?}", b.logs);
        assert!(b.logs.iter().any(|l| l.contains("cancelled=true")), "{:?}", b.logs);
        assert!(b.logs.iter().any(|l| l.contains("final canceled")), "{:?}", b.logs);
    }

    #[test]
    fn a_broker_refusal_is_a_note_and_a_rejected_order_not_a_crash() {
        let src = "machine M { param sym: Symbol; param account: Account = SandBox; init { launch (p, A); }
            process p { let o: Order; listen Tickers[sym];
              state A: goto B { o = account.sell_limit(sym, 5, 120); }
              state B { printf(\"%s\", o.status()); } } }";
        let b = run(src, &[100.0, 100.0, 100.0], cfg(serde_json::json!({"sym": "TEST"}))).unwrap();
        assert!(b.logs.iter().any(|l| l.contains("rejected")), "{:?}", b.logs);
    }

    #[test]
    fn engines_can_run_on_any_broker_and_the_orders_reach_it() {
        use std::sync::{Arc as A, Mutex};
        struct Spy(bk::PaperBroker, A<Mutex<Vec<String>>>);
        impl Broker for Spy {
            fn name(&self) -> &str { "spy" }
            fn environment(&self) -> bk::Environment { bk::Environment::Paper }
            fn account(&mut self) -> bk::Result<bk::Account> { self.0.account() }
            fn positions(&mut self) -> bk::Result<Vec<bk::Position>> { self.0.positions() }
            fn quote(&mut self, s: &str) -> bk::Result<bk::Quote> { self.0.quote(s) }
            fn place_order(&mut self, r: &bk::OrderRequest) -> bk::Result<bk::Order> {
                self.1.lock().unwrap().push(format!("{} {} {} {}", r.client_id, r.side.as_str(), r.qty, r.symbol));
                self.0.place_order(r)
            }
            fn cancel_order(&mut self, id: &str) -> bk::Result<bk::Order> { self.0.cancel_order(id) }
            fn order(&mut self, id: &str) -> bk::Result<bk::Order> { self.0.order(id) }
            fn open_orders(&mut self) -> bk::Result<Vec<bk::Order>> { self.0.open_orders() }
            fn on_bar(&mut self, c: &BTreeMap<String, bk::Candle>) { self.0.on_bar(c) }
        }
        let seen = A::new(Mutex::new(vec![]));
        let broker = Spy(bk::PaperBroker::new(10_000.0, 0.0), seen.clone());
        let c = Config { params: serde_json::from_value(serde_json::json!({"floor": 10, "ceiling": 15})).unwrap(), client_prefix: Some("dep1".into()), ..Default::default() };
        let mut e = Engine::with_broker(Arc::new(parse(THRESHOLD).unwrap()), &["TEST".to_string()], 86_400.0, &c, Box::new(broker)).unwrap();
        for (i, p) in [12.0, 9.0, 9.0, 9.0, 16.0, 17.0].iter().enumerate() {
            e.step(&bar_of(*p, i)).unwrap();
        }
        let log = seen.lock().unwrap().clone();
        assert_eq!(log.len(), 2, "{log:?}");
        assert!(log[0].starts_with("dep1-1 buy ") && log[1].starts_with("dep1-2 sell "), "{log:?}");
        assert!(e.cash() > 10_000.0, "the sale at 17 left a profit in the broker's account");
    }
}
