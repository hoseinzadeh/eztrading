//! Declarative strategy tests: a price path, optional parameter overrides, and
//! the behaviour you expect (trades, final position, final state, ...).
//!
//! ```json
//! { "tests": [ {
//!     "name": "buys below the floor and sells above the ceiling",
//!     "prices": [12, 11, 9, 9.5, 16, 17],
//!     "params": { "floor": 10, "ceiling": 15 },
//!     "expect": {
//!       "trades": [ { "bar": 3, "side": "buy" }, { "bar": 5, "side": "sell" } ],
//!       "final_shares": 0,
//!       "final_state": { "watch(TEST)": "WAIT" }
//!     }
//! } ] }
//! ```

use crate::model::Strategy;
use crate::sim::{self, Backtest, Config};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const MAX_TESTS: usize = 200;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct TestFile {
    pub tests: Vec<TestCase>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct TestCase {
    pub name: String,
    #[serde(default = "default_symbol", skip_serializing_if = "is_default_symbol")]
    pub symbol: String,
    pub prices: Vec<f64>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine: Option<String>,
    #[serde(default)]
    pub expect: Expect,
}

fn default_symbol() -> String {
    "TEST".into()
}

fn is_default_symbol(s: &String) -> bool {
    s == "TEST"
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct Expect {
    /// The exact sequence of fills. Only the fields you give are compared.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trades: Option<Vec<ExpectTrade>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_trades: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_trades: Option<usize>,
    /// Shares held of the test symbol after the last bar.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_shares: Option<f64>,
    /// Final state per process instance, e.g. `{"watch(TEST)": "WAIT"}`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_state: Option<BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_equity_at_least: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_drawdown_pct_at_most: Option<f64>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct ExpectTrade {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bar: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub side: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shares: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub price: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TestResult {
    pub name: String,
    pub passed: bool,
    pub failures: Vec<String>,
    /// Set when the strategy could not be simulated at all.
    pub error: Option<String>,
    pub trades: usize,
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-6 * a.abs().max(b.abs()).max(1.0)
}

pub fn parse_tests(text: &str) -> Result<TestFile, String> {
    let file: TestFile = serde_json::from_str(text).map_err(|e| format!("invalid test file: {e}"))?;
    if file.tests.is_empty() {
        return Err("the test file has no tests".into());
    }
    if file.tests.len() > MAX_TESTS {
        return Err(format!("at most {MAX_TESTS} tests are supported"));
    }
    Ok(file)
}

pub fn run_tests(strategy: &Strategy, file: &TestFile) -> Vec<TestResult> {
    file.tests.iter().map(|t| run_one(strategy, t)).collect()
}

fn run_one(strategy: &Strategy, t: &TestCase) -> TestResult {
    let market = sim::market_from_closes(&t.symbol, &t.prices, 86_400.0);
    let cfg = Config { machine: t.machine.clone(), params: t.params.clone(), ..Default::default() };
    match sim::backtest(strategy, &market, &cfg) {
        Err(e) => TestResult { name: t.name.clone(), passed: false, failures: vec![], error: Some(e.to_string()), trades: 0 },
        Ok(b) => {
            let failures = check(&t.expect, &b);
            TestResult { name: t.name.clone(), passed: failures.is_empty(), failures, error: None, trades: b.trades.len() }
        }
    }
}

fn check(e: &Expect, b: &Backtest) -> Vec<String> {
    let mut f = vec![];
    if let Some(expected) = &e.trades {
        if expected.len() != b.trades.len() {
            f.push(format!("expected {} trade(s) but the strategy made {}: [{}]", expected.len(), b.trades.len(), describe(b)));
        }
        for (i, (want, got)) in expected.iter().zip(&b.trades).enumerate() {
            let n = i + 1;
            if let Some(bar) = want.bar {
                if bar != got.bar {
                    f.push(format!("trade {n}: expected it on bar {bar} but it happened on bar {}", got.bar));
                }
            }
            if let Some(side) = &want.side {
                if side != got.side {
                    f.push(format!("trade {n}: expected a {side} but it was a {}", got.side));
                }
            }
            if let Some(shares) = want.shares {
                if !close(shares, got.shares) {
                    f.push(format!("trade {n}: expected {shares} shares but got {:.6}", got.shares));
                }
            }
            if let Some(price) = want.price {
                if !close(price, got.price) {
                    f.push(format!("trade {n}: expected price {price} but got {}", got.price));
                }
            }
        }
    }
    if let Some(min) = e.min_trades {
        if b.trades.len() < min {
            f.push(format!("expected at least {min} trade(s) but got {}", b.trades.len()));
        }
    }
    if let Some(max) = e.max_trades {
        if b.trades.len() > max {
            f.push(format!("expected at most {max} trade(s) but got {}", b.trades.len()));
        }
    }
    if let Some(want) = e.final_shares {
        let got = b.position.last().copied().unwrap_or(0.0);
        if !close(want, got) {
            f.push(format!("expected {want} shares at the end but held {got:.6}"));
        }
    }
    if let Some(want) = &e.final_state {
        for (proc, state) in want {
            match b.final_states.get(proc) {
                None => f.push(format!("no process instance `{proc}` (instances: {})", b.final_states.keys().cloned().collect::<Vec<_>>().join(", "))),
                Some(got) if got != state => f.push(format!("expected `{proc}` to end in state {state} but it ended in {got}")),
                _ => {}
            }
        }
    }
    if let Some(min) = e.final_equity_at_least {
        if b.stats.final_equity < min {
            f.push(format!("final equity {:.2} is below {min}", b.stats.final_equity));
        }
    }
    if let Some(max) = e.max_drawdown_pct_at_most {
        if b.stats.max_drawdown_pct > max {
            f.push(format!("max drawdown {:.2}% exceeds {max}%", b.stats.max_drawdown_pct));
        }
    }
    f
}

fn describe(b: &Backtest) -> String {
    b.trades.iter().map(|t| format!("{} on bar {}", t.side, t.bar)).collect::<Vec<_>>().join(", ")
}

/// Turns a backtest into a regression test that pins its trades and final state.
pub fn golden(name: &str, symbol: &str, prices: Vec<f64>, params: BTreeMap<String, serde_json::Value>, machine: Option<String>, b: &Backtest) -> TestCase {
    let trades = b
        .trades
        .iter()
        .map(|t| ExpectTrade { bar: Some(t.bar), side: Some(t.side.to_owned()), shares: None, price: None })
        .collect();
    TestCase {
        name: name.to_owned(),
        symbol: symbol.to_owned(),
        prices,
        params,
        machine,
        expect: Expect {
            trades: Some(trades),
            final_shares: Some(b.position.last().copied().unwrap_or(0.0)),
            final_state: Some(b.final_states.clone()),
            ..Default::default()
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::parse;

    const THRESHOLD: &str = include_str!("../../../examples/threshold.sm");

    fn run(json: &str) -> Vec<TestResult> {
        run_tests(&parse(THRESHOLD).unwrap(), &parse_tests(json).unwrap())
    }

    const GOOD: &str = r#"{ "tests": [ {
        "name": "round trip",
        "prices": [12, 11, 9, 9.5, 16, 17],
        "params": { "floor": 10, "ceiling": 15 },
        "expect": { "trades": [ {"bar": 3, "side": "buy", "price": 9.5}, {"bar": 5, "side": "sell"} ],
                    "final_shares": 0, "final_state": { "watch(TEST)": "WAIT" }, "min_trades": 2, "max_trades": 2 }
    } ] }"#;

    #[test]
    fn passing_test() {
        let r = run(GOOD);
        assert!(r[0].passed, "{:?}", r[0]);
        assert_eq!(r[0].trades, 2);
    }

    #[test]
    fn failures_explain_the_difference() {
        let r = run(r#"{ "tests": [ {
            "name": "wrong expectations",
            "prices": [12, 11, 9, 9.5, 16, 17],
            "params": { "floor": 10, "ceiling": 15 },
            "expect": { "trades": [ {"bar": 2, "side": "sell"} ], "final_shares": 5, "final_state": { "watch(TEST)": "HOLD", "nope": "X" } }
        } ] }"#);
        assert!(!r[0].passed);
        let all = r[0].failures.join("\n");
        assert!(all.contains("expected 1 trade(s) but the strategy made 2"), "{all}");
        assert!(all.contains("expected it on bar 2 but it happened on bar 3"), "{all}");
        assert!(all.contains("expected a sell but it was a buy"), "{all}");
        assert!(all.contains("expected 5 shares at the end"), "{all}");
        assert!(all.contains("ended in WAIT"), "{all}");
        assert!(all.contains("no process instance `nope`"), "{all}");
    }

    #[test]
    fn simulation_errors_are_reported_per_test() {
        let r = run(r#"{ "tests": [ { "name": "bad", "prices": [1], "params": {}, "machine": "Nope" } ] }"#);
        assert!(!r[0].passed);
        assert!(r[0].error.as_deref().unwrap().contains("Nope"));
    }

    #[test]
    fn empty_expectations_pass_if_the_strategy_runs() {
        let r = run(r#"{ "tests": [ { "name": "smoke", "prices": [12, 13, 14] } ] }"#);
        assert!(r[0].passed);
    }

    #[test]
    fn rejects_malformed_files() {
        assert!(parse_tests("not json").unwrap_err().starts_with("invalid test file"));
        assert!(parse_tests(r#"{"tests": []}"#).unwrap_err().contains("no tests"));
        assert!(parse_tests(r#"{"tests": [{"name": "x"}]}"#).is_err()); // prices required
    }

    #[test]
    fn golden_test_captures_behaviour_and_passes_against_the_same_strategy() {
        let s = parse(THRESHOLD).unwrap();
        let prices = vec![12.0, 11.0, 9.0, 9.5, 16.0, 17.0];
        let params: BTreeMap<String, serde_json::Value> = serde_json::from_str(r#"{"floor": 10, "ceiling": 15}"#).unwrap();
        let b = sim::backtest(&s, &sim::market_from_closes("TEST", &prices, 86_400.0), &Config { params: params.clone(), ..Default::default() }).unwrap();
        let t = golden("captured", "TEST", prices, params, None, &b);
        let file = TestFile { tests: vec![t] };
        // Round-trips through JSON and passes.
        let json = serde_json::to_string(&file).unwrap();
        let again = parse_tests(&json).unwrap();
        assert_eq!(again, file);
        assert!(run_tests(&s, &again)[0].passed);
        // A different strategy behaviour (other thresholds) now fails the pinned test.
        let mut changed = file.clone();
        changed.tests[0].params.insert("floor".into(), serde_json::json!(5));
        assert!(!run_tests(&s, &changed)[0].passed);
    }
}
