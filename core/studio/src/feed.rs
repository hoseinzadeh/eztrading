//! Market-data feeds for live (paper) deployments. A feed delivers one bar per
//! symbol each time it is asked; the deployment's thread paces the asking.

use crate::market::{self, Bar, GenStream, Generator, MAX_BARS, MAX_SYMBOLS};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::Duration;

pub const MIN_TICK_MS: u64 = 20;
pub const MAX_TICK_MS: u64 = 3_600_000;

fn default_symbol() -> String {
    "TEST".into()
}

fn default_tick() -> u64 {
    1_000
}

fn default_interval() -> u64 {
    60
}

/// Where a deployment's prices come from. Stored with the deployment.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FeedSpec {
    /// Plays a historical series back one bar per tick; the deployment finishes when it ends.
    Replay {
        #[serde(default = "default_symbol")]
        symbol: String,
        data: ReplayData,
        #[serde(default = "default_tick")]
        tick_ms: u64,
    },
    /// An endless synthetic stream (deterministic for a seed), one bar per tick.
    Simulated {
        #[serde(default = "default_symbol")]
        symbol: String,
        #[serde(default)]
        generator: Generator,
        #[serde(default = "default_tick")]
        tick_ms: u64,
    },
    /// The latest price from Twelve Data, polled every `interval_secs` seconds
    /// (needs `TWELVEDATA_API_KEY` in the server's environment).
    TwelveData {
        symbols: Vec<String>,
        #[serde(default = "default_interval")]
        interval_secs: u64,
    },
    /// Quotes from a brokerage connection (`brokers/<name>.json`), polled every `tick_ms`.
    /// The bar is the sampled last price. Uses its own read-only connection.
    Broker {
        broker: String,
        symbols: Vec<String>,
        #[serde(default = "default_broker_tick")]
        tick_ms: u64,
    },
}

fn default_broker_tick() -> u64 {
    60_000
}

pub const MIN_BROKER_TICK_MS: u64 = 250;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum ReplayData {
    Generate { generator: Generator },
    Csv { csv: String },
}

impl FeedSpec {
    pub fn symbols(&self) -> Vec<String> {
        match self {
            FeedSpec::Replay { symbol, .. } | FeedSpec::Simulated { symbol, .. } => vec![symbol.clone()],
            FeedSpec::TwelveData { symbols, .. } | FeedSpec::Broker { symbols, .. } => symbols.clone(),
        }
    }

    /// Wall-clock time between bars.
    pub fn tick(&self) -> Duration {
        match self {
            FeedSpec::Replay { tick_ms, .. } | FeedSpec::Simulated { tick_ms, .. } => Duration::from_millis(*tick_ms),
            FeedSpec::TwelveData { interval_secs, .. } => Duration::from_secs(*interval_secs),
            FeedSpec::Broker { tick_ms, .. } => Duration::from_millis(*tick_ms),
        }
    }

    /// A one-line description for lists.
    pub fn describe(&self) -> String {
        match self {
            FeedSpec::Replay { symbol, data, tick_ms } => {
                let what = match data {
                    ReplayData::Generate { generator } => format!("{} replay, {} bars", generator.kind, generator.bars),
                    ReplayData::Csv { .. } => "CSV replay".to_owned(),
                };
                format!("{symbol}: {what}, one bar per {}", fmt_ms(*tick_ms))
            }
            FeedSpec::Simulated { symbol, generator, tick_ms } => format!("{symbol}: simulated {} stream, one bar per {}", generator.kind, fmt_ms(*tick_ms)),
            FeedSpec::TwelveData { symbols, interval_secs, .. } => format!("{}: Twelve Data every {interval_secs}s", symbols.join(", ")),
            FeedSpec::Broker { broker, symbols, tick_ms } => format!("{}: quotes from {broker} every {}", symbols.join(", "), fmt_ms(*tick_ms)),
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        let symbols = self.symbols();
        if symbols.is_empty() || symbols.len() > MAX_SYMBOLS {
            return Err(format!("between 1 and {MAX_SYMBOLS} symbols are required"));
        }
        for s in &symbols {
            if !valid_symbol(s) {
                return Err(format!("invalid symbol `{s}` (letters, digits and `. : / -`, up to 20 characters)"));
            }
        }
        match self {
            FeedSpec::Replay { tick_ms, .. } | FeedSpec::Simulated { tick_ms, .. } => {
                if !(MIN_TICK_MS..=MAX_TICK_MS).contains(tick_ms) {
                    return Err(format!("tick_ms must be between {MIN_TICK_MS} and {MAX_TICK_MS}"));
                }
            }
            FeedSpec::TwelveData { interval_secs, .. } => {
                if !(5..=86_400).contains(interval_secs) {
                    return Err("interval_secs must be between 5 and 86400".into());
                }
            }
            FeedSpec::Broker { broker, tick_ms, .. } => {
                if !crate::brokers::valid_name(broker) {
                    return Err("the broker feed needs the name of a connection".into());
                }
                if !(MIN_BROKER_TICK_MS..=MAX_TICK_MS).contains(tick_ms) {
                    return Err(format!("tick_ms must be between {MIN_BROKER_TICK_MS} and {MAX_TICK_MS}"));
                }
            }
        }
        Ok(())
    }
}

fn fmt_ms(ms: u64) -> String {
    if ms >= 1000 && ms % 1000 == 0 { format!("{}s", ms / 1000) } else { format!("{ms}ms") }
}

pub fn valid_symbol(s: &str) -> bool {
    !s.is_empty() && s.len() <= 20 && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | ':' | '/' | '-'))
}

pub enum Poll {
    Bars(BTreeMap<String, Bar>),
    /// The feed has no more data (a finished replay).
    End,
    /// A transient failure; asking again later may succeed.
    Failed(String),
}

pub trait Feed: Send {
    fn next(&mut self) -> Poll;
    /// Fast-forwards over `n` bars that were already delivered before a restart.
    fn skip(&mut self, n: usize);
}

/// Builds the feed for a spec. `bar_secs` labels synthetic bars; the Twelve Data
/// key is read from the environment here and never stored.
pub fn open(spec: &FeedSpec, bar_secs: f64) -> Result<Box<dyn Feed>, String> {
    open_with(spec, bar_secs, None)
}

/// Like `open`, with the registry that broker quote feeds connect through.
pub fn open_with(spec: &FeedSpec, bar_secs: f64, brokers: Option<&crate::brokers::Brokers>) -> Result<Box<dyn Feed>, String> {
    spec.validate()?;
    Ok(match spec {
        FeedSpec::Replay { symbol, data, .. } => {
            let bars = match data {
                ReplayData::Generate { generator } => market::generate(generator, bar_secs)?,
                ReplayData::Csv { csv } => market::parse_csv(csv, bar_secs)?,
            };
            if bars.len() > MAX_BARS {
                return Err(format!("at most {MAX_BARS} bars"));
            }
            Box::new(ReplayFeed { symbol: symbol.clone(), bars, pos: 0 })
        }
        FeedSpec::Simulated { symbol, generator, .. } => Box::new(SimFeed { symbol: symbol.clone(), stream: GenStream::new(generator, bar_secs)? }),
        FeedSpec::TwelveData { symbols, .. } => {
            let key = std::env::var("TWELVEDATA_API_KEY").ok().filter(|k| !k.trim().is_empty()).ok_or("TWELVEDATA_API_KEY is not set in the server's environment")?;
            let base = std::env::var("TWELVEDATA_BASE_URL").unwrap_or_else(|_| "https://api.twelvedata.com".into());
            Box::new(TwelveDataFeed::new(symbols.clone(), key, base)?)
        }
        FeedSpec::Broker { broker, symbols, .. } => {
            let reg = brokers.ok_or("broker quote feeds are not available here")?;
            Box::new(BrokerFeed { broker: Box::new(reg.connect(broker)?), symbols: symbols.clone() })
        }
    })
}

/// Samples the last price from a brokerage. The connection builds its HTTP client on first use,
/// on the deployment's own thread.
pub struct BrokerFeed {
    broker: Box<dyn ezt_broker::Broker>,
    symbols: Vec<String>,
}

impl Feed for BrokerFeed {
    fn next(&mut self) -> Poll {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
        let label = market::iso_timestamp(now);
        let mut bars = BTreeMap::new();
        for sym in &self.symbols {
            match self.broker.quote(sym) {
                Ok(q) if q.last.is_finite() && q.last > 0.0 => {
                    bars.insert(sym.clone(), Bar { label: label.clone(), open: q.last, high: q.last, low: q.last, close: q.last, volume: 0.0 });
                }
                Ok(_) => return Poll::Failed(format!("{sym}: the quote has no valid price")),
                Err(e) => return Poll::Failed(format!("{sym}: {}", e.message)),
            }
        }
        Poll::Bars(bars)
    }

    fn skip(&mut self, _n: usize) {}
}

pub struct ReplayFeed {
    symbol: String,
    bars: Vec<Bar>,
    pos: usize,
}

impl Feed for ReplayFeed {
    fn next(&mut self) -> Poll {
        match self.bars.get(self.pos) {
            Some(b) => {
                self.pos += 1;
                Poll::Bars(BTreeMap::from([(self.symbol.clone(), b.clone())]))
            }
            None => Poll::End,
        }
    }

    fn skip(&mut self, n: usize) {
        self.pos = (self.pos + n).min(self.bars.len());
    }
}

pub struct SimFeed {
    symbol: String,
    stream: GenStream,
}

impl Feed for SimFeed {
    fn next(&mut self) -> Poll {
        Poll::Bars(BTreeMap::from([(self.symbol.clone(), self.stream.next_bar())]))
    }

    fn skip(&mut self, n: usize) {
        for _ in 0..n.min(MAX_BARS * 5) {
            self.stream.next_bar();
        }
    }
}

pub struct TwelveDataFeed {
    /// Built on first use, on the deployment's own thread: `reqwest::blocking` panics if its
    /// client is created or dropped inside an async runtime (e.g. while restoring at startup).
    http: Option<reqwest::blocking::Client>,
    symbols: Vec<String>,
    key: String,
    base: String,
}

impl TwelveDataFeed {
    pub fn new(symbols: Vec<String>, key: String, base: String) -> Result<Self, String> {
        Ok(Self { http: None, symbols, key, base: base.trim_end_matches('/').to_owned() })
    }

    fn client(&mut self) -> Result<&reqwest::blocking::Client, String> {
        if self.http.is_none() {
            let c = reqwest::blocking::Client::builder().timeout(Duration::from_secs(10)).build().map_err(|e| e.to_string())?;
            self.http = Some(c);
        }
        Ok(self.http.as_ref().expect("just set"))
    }

    fn price(&mut self, symbol: &str) -> Result<f64, String> {
        let (base, key) = (self.base.clone(), self.key.clone());
        let resp = self
            .client()?
            .get(format!("{base}/price"))
            .query(&[("symbol", symbol), ("apikey", key.as_str())])
            .send()
            // `without_url`: the request URL contains the API key.
            .map_err(|e| format!("{symbol}: request failed: {}", e.without_url()))?;
        let status = resp.status();
        let body: serde_json::Value = resp.json().map_err(|e| format!("{symbol}: unreadable response (HTTP {status}): {}", e.without_url()))?;
        if body["status"] == "error" || !status.is_success() {
            let msg = body["message"].as_str().unwrap_or("request rejected");
            return Err(format!("{symbol}: {msg}"));
        }
        let price = match &body["price"] {
            serde_json::Value::String(s) => s.parse::<f64>().ok(),
            serde_json::Value::Number(n) => n.as_f64(),
            _ => None,
        };
        price.filter(|p| p.is_finite() && *p > 0.0).ok_or_else(|| format!("{symbol}: the response has no valid price"))
    }
}

impl Feed for TwelveDataFeed {
    fn next(&mut self) -> Poll {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
        let label = market::iso_timestamp(now);
        let mut bars = BTreeMap::new();
        for sym in self.symbols.clone() {
            match self.price(&sym) {
                Ok(p) => {
                    bars.insert(sym, Bar { label: label.clone(), open: p, high: p, low: p, close: p, volume: 0.0 });
                }
                Err(e) => return Poll::Failed(e),
            }
        }
        Poll::Bars(bars)
    }

    fn skip(&mut self, _n: usize) {
        // History comes from the deployment's own log; live quotes cannot be replayed.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    fn gen(bars: usize) -> Generator {
        Generator { bars, seed: 5, ..Default::default() }
    }

    fn closes(p: Poll) -> Vec<f64> {
        match p {
            Poll::Bars(b) => b.values().map(|x| x.close).collect(),
            Poll::End => panic!("end"),
            Poll::Failed(e) => panic!("{e}"),
        }
    }

    #[test]
    fn replay_delivers_every_bar_then_ends_and_can_skip() {
        let spec = FeedSpec::Replay { symbol: "X".into(), data: ReplayData::Generate { generator: gen(5) }, tick_ms: 20 };
        let mut f = open(&spec, 86_400.0).unwrap();
        let all: Vec<f64> = (0..5).map(|_| closes(f.next())[0]).collect();
        assert!(matches!(f.next(), Poll::End));
        let mut g = open(&spec, 86_400.0).unwrap();
        g.skip(3);
        assert_eq!(closes(g.next())[0], all[3]);
        g.skip(100);
        assert!(matches!(g.next(), Poll::End));
    }

    #[test]
    fn simulated_stream_is_endless_deterministic_and_resumable() {
        let spec = FeedSpec::Simulated { symbol: "X".into(), generator: gen(10), tick_ms: 20 };
        let mut a = open(&spec, 86_400.0).unwrap();
        let first: Vec<f64> = (0..30).map(|_| closes(a.next())[0]).collect();
        let mut b = open(&spec, 86_400.0).unwrap();
        b.skip(25);
        assert_eq!(closes(b.next())[0], first[25], "skipping reproduces the same stream");
    }

    #[test]
    fn csv_replay() {
        let spec = FeedSpec::Replay { symbol: "X".into(), data: ReplayData::Csv { csv: "close\n10\n11\n".into() }, tick_ms: 20 };
        let mut f = open(&spec, 86_400.0).unwrap();
        assert_eq!(closes(f.next()), [10.0]);
        assert_eq!(closes(f.next()), [11.0]);
        assert!(matches!(f.next(), Poll::End));
        let bad = FeedSpec::Replay { symbol: "X".into(), data: ReplayData::Csv { csv: "abc".into() }, tick_ms: 20 };
        assert!(open(&bad, 86_400.0).is_err());
    }

    #[test]
    fn specs_round_trip_and_validate() {
        let json = r#"{"kind":"replay","symbol":"AAPL","data":{"source":"generate","generator":{"bars":50,"seed":2}},"tick_ms":50}"#;
        let spec: FeedSpec = serde_json::from_str(json).unwrap();
        assert_eq!(spec.symbols(), ["AAPL"]);
        assert_eq!(spec.tick(), Duration::from_millis(50));
        assert_eq!(serde_json::from_str::<FeedSpec>(&serde_json::to_string(&spec).unwrap()).unwrap(), spec);
        assert!(spec.describe().contains("replay"));
        let sim: FeedSpec = serde_json::from_str(r#"{"kind":"simulated"}"#).unwrap();
        assert_eq!((sim.symbols(), sim.tick()), (vec!["TEST".to_string()], Duration::from_secs(1)));

        for (bad, why) in [
            (r#"{"kind":"simulated","tick_ms":5}"#, "tick_ms"),
            (r#"{"kind":"simulated","symbol":"a b"}"#, "invalid symbol"),
            (r#"{"kind":"twelve_data","symbols":[]}"#, "symbols"),
            (r#"{"kind":"twelve_data","symbols":["A"],"interval_secs":1}"#, "interval_secs"),
        ] {
            let spec: FeedSpec = serde_json::from_str(bad).unwrap();
            assert!(spec.validate().unwrap_err().contains(why), "{bad}");
        }
        assert!(serde_json::from_str::<FeedSpec>(r#"{"kind":"nope"}"#).is_err());
    }

    /// A tiny HTTP server answering `/price?symbol=...` for the Twelve Data tests.
    fn fake_twelvedata(requests: usize) -> (String, std::thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            let mut seen = vec![];
            for _ in 0..requests {
                let (mut sock, _) = listener.accept().unwrap();
                let mut buf = [0u8; 4096];
                let n = sock.read(&mut buf).unwrap();
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let line = req.lines().next().unwrap_or("").to_owned();
                let body = if line.contains("symbol=BAD") {
                    r#"{"code":400,"message":"symbol not found","status":"error"}"#.to_owned()
                } else if line.contains("symbol=AAPL") {
                    r#"{"price":"189.3200"}"#.to_owned()
                } else {
                    r#"{"price":101.5}"#.to_owned()
                };
                let resp = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                sock.write_all(resp.as_bytes()).unwrap();
                seen.push(line);
            }
            seen
        });
        (base, handle)
    }

    #[test]
    fn twelve_data_polls_prices_and_reports_api_errors_without_leaking_the_key() {
        let (base, server) = fake_twelvedata(3);
        let mut f = TwelveDataFeed::new(vec!["AAPL".into(), "MSFT".into()], "SECRET-KEY".into(), base.clone()).unwrap();
        match f.next() {
            Poll::Bars(b) => {
                assert_eq!(b["AAPL"].close, 189.32);
                assert_eq!(b["MSFT"].close, 101.5);
                assert!(b["AAPL"].label.ends_with('Z') && b["AAPL"].label.contains('T'));
            }
            _ => panic!("expected bars"),
        }
        let mut bad = TwelveDataFeed::new(vec!["BAD".into()], "SECRET-KEY".into(), base).unwrap();
        match bad.next() {
            Poll::Failed(e) => assert_eq!(e, "BAD: symbol not found"),
            _ => panic!("expected failure"),
        }
        let lines = server.join().unwrap();
        assert!(lines[0].starts_with("GET /price?symbol=AAPL&apikey=SECRET-KEY"), "{}", lines[0]);
    }

    #[test]
    fn twelve_data_connection_errors_do_not_contain_the_key() {
        let mut f = TwelveDataFeed::new(vec!["AAPL".into()], "SECRET-KEY".into(), "http://127.0.0.1:1".into()).unwrap();
        match f.next() {
            Poll::Failed(e) => {
                assert!(e.starts_with("AAPL: request failed"), "{e}");
                assert!(!e.contains("SECRET-KEY") && !e.contains("apikey"), "{e}");
            }
            _ => panic!("expected failure"),
        }
    }

    #[test]
    fn creating_and_dropping_a_feed_inside_an_async_runtime_does_not_panic() {
        // Restoring deployments at startup happens inside the web server's runtime.
        actix_web::rt::System::new().block_on(async {
            let spec = FeedSpec::TwelveData { symbols: vec!["AAPL".into()], interval_secs: 60 };
            let feed = TwelveDataFeed::new(vec!["AAPL".into()], "k".into(), "http://127.0.0.1:1".into()).unwrap();
            drop(feed);
            assert!(spec.validate().is_ok());
        });
    }

    #[test]
    fn twelve_data_requires_a_key_in_the_environment() {
        let spec = FeedSpec::TwelveData { symbols: vec!["AAPL".into()], interval_secs: 60 };
        std::env::remove_var("TWELVEDATA_API_KEY");
        assert!(open(&spec, 60.0).err().unwrap().contains("TWELVEDATA_API_KEY"));
    }
}
