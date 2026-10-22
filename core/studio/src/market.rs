//! Price data for backtests: CSV parsing and deterministic synthetic generators.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const MAX_BARS: usize = 20_000;
pub const MAX_SYMBOLS: usize = 20;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Bar {
    pub label: String,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
}

/// Aligned price series (same number of bars for every symbol).
#[derive(Debug, Clone)]
pub struct Market {
    pub bar_secs: f64,
    pub series: BTreeMap<String, Vec<Bar>>,
}

impl Market {
    pub fn single(symbol: &str, bars: Vec<Bar>, bar_secs: f64) -> Self {
        Self { bar_secs, series: BTreeMap::from([(symbol.to_owned(), bars)]) }
    }

    pub fn len(&self) -> usize {
        self.series.values().next().map_or(0, Vec::len)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.series.is_empty() {
            return Err("no price data".into());
        }
        if self.series.len() > MAX_SYMBOLS {
            return Err(format!("at most {MAX_SYMBOLS} symbols are supported"));
        }
        if !(self.bar_secs.is_finite() && self.bar_secs >= 1.0) {
            return Err("bar_seconds must be at least 1".into());
        }
        let n = self.len();
        if n == 0 {
            return Err("the price series is empty".into());
        }
        if n > MAX_BARS {
            return Err(format!("at most {MAX_BARS} bars are supported (got {n})"));
        }
        for (sym, bars) in &self.series {
            if bars.len() != n {
                return Err(format!("symbol {sym} has {} bars but the others have {n}", bars.len()));
            }
            if let Some(i) = bars.iter().position(|b| !(b.close.is_finite() && b.close > 0.0)) {
                return Err(format!("{sym}: bar {i} has a non-positive or invalid close"));
            }
        }
        Ok(())
    }

    pub fn symbols(&self) -> Vec<String> {
        self.series.keys().cloned().collect()
    }
}

/// Builds bars from close prices only (used by tests and the generators' labels).
pub fn bars_from_closes(closes: &[f64], bar_secs: f64) -> Vec<Bar> {
    let mut prev = closes.first().copied().unwrap_or(0.0);
    closes
        .iter()
        .enumerate()
        .map(|(i, &c)| {
            let open = prev;
            prev = c;
            Bar { label: label(i, bar_secs), open, high: open.max(c), low: open.min(c), close: c, volume: 0.0 }
        })
        .collect()
}

/// Calendar label of bar `i`: ISO dates for daily-or-slower bars, `t+i` otherwise.
pub fn label(i: usize, bar_secs: f64) -> String {
    if bar_secs >= 86_400.0 {
        let days = (i as f64 * bar_secs / 86_400.0) as i64;
        let (y, m, d) = civil_from_days(days_from_civil(2024, 1, 1) + days);
        format!("{y:04}-{m:02}-{d:02}")
    } else {
        format!("t+{i}")
    }
}

// Howard Hinnant's civil-date algorithms.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Parses `date,open,high,low,close,volume`, `date,close` or a bare `close` column.
/// A header row is detected automatically; blank lines and `#` comments are skipped.
pub fn parse_csv(text: &str, bar_secs: f64) -> Result<Vec<Bar>, String> {
    let mut bars = vec![];
    let mut header: Option<Vec<String>> = None;
    for (n, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let cells: Vec<&str> = line.split([',', ';', '\t']).map(str::trim).collect();
        let numeric = |s: &str| s.parse::<f64>().is_ok();
        if bars.is_empty() && header.is_none() && cells.iter().all(|c| !numeric(c)) {
            header = Some(cells.iter().map(|c| c.to_lowercase()).collect());
            continue;
        }
        let col = |name: &str| header.as_ref().and_then(|h| h.iter().position(|c| c == name));
        let get = |i: usize| -> Result<f64, String> {
            cells.get(i).ok_or_else(|| format!("line {}: missing column {}", n + 1, i + 1))?.parse::<f64>().map_err(|_| format!("line {}: `{}` is not a number", n + 1, cells[i]))
        };
        let bar = if let (Some(c), true) = (col("close"), header.is_some()) {
            let date_col = col("date").or_else(|| col("time")).or_else(|| col("timestamp"));
            let close = get(c)?;
            let pick = |name: &str| col(name).map(&get).transpose();
            Bar {
                label: date_col.and_then(|d| cells.get(d)).map(|s| s.to_string()).unwrap_or_else(|| label(bars.len(), bar_secs)),
                open: pick("open")?.unwrap_or(close),
                high: pick("high")?.unwrap_or(close),
                low: pick("low")?.unwrap_or(close),
                close,
                volume: pick("volume")?.unwrap_or(0.0),
            }
        } else {
            match cells.len() {
                1 => {
                    let close = get(0)?;
                    Bar { label: label(bars.len(), bar_secs), open: close, high: close, low: close, close, volume: 0.0 }
                }
                2 => {
                    let close = get(1)?;
                    Bar { label: cells[0].to_owned(), open: close, high: close, low: close, close, volume: 0.0 }
                }
                6.. => Bar { label: cells[0].to_owned(), open: get(1)?, high: get(2)?, low: get(3)?, close: get(4)?, volume: get(5)? },
                _ => return Err(format!("line {}: expected 1, 2 or 6 columns (close | date,close | date,open,high,low,close,volume)", n + 1)),
            }
        };
        bars.push(bar);
        if bars.len() > MAX_BARS {
            return Err(format!("more than {MAX_BARS} bars"));
        }
    }
    if bars.is_empty() {
        return Err("the CSV has no data rows".into());
    }
    Ok(bars)
}

/// Parameters of a synthetic series. Every field has a default.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Generator {
    /// `random_walk`, `trend_up`, `trend_down`, `sine` or `crash`.
    pub kind: String,
    pub seed: u64,
    pub bars: usize,
    pub start: f64,
    /// Mean log-return per bar (random_walk, crash).
    pub drift: f64,
    /// Standard deviation of the log-return per bar.
    pub vol: f64,
    /// Cycle length in bars (sine).
    pub period: usize,
    /// Relative swing around the start price (sine).
    pub amplitude: f64,
}

impl Default for Generator {
    fn default() -> Self {
        Self { kind: "random_walk".into(), seed: 1, bars: 250, start: 100.0, drift: 0.0003, vol: 0.015, period: 60, amplitude: 0.25 }
    }
}

struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        // splitmix64
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn uniform(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Standard normal (Box-Muller).
    fn normal(&mut self) -> f64 {
        let (u1, u2) = (self.uniform().max(1e-12), self.uniform());
        (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
    }
}

/// A synthetic price series produced one bar at a time (so it can run forever).
/// `generate` is this, collected; the two give identical bars for the same seed.
pub struct GenStream {
    g: Generator,
    rng: Rng,
    close: f64,
    i: usize,
    drift: f64,
    vol: f64,
    bar_secs: f64,
}

impl GenStream {
    pub fn new(g: &Generator, bar_secs: f64) -> Result<Self, String> {
        if !(g.start.is_finite() && g.start > 0.0) || !g.vol.is_finite() || g.vol < 0.0 || !g.drift.is_finite() {
            return Err("start must be positive and vol non-negative".into());
        }
        let (drift, vol) = match g.kind.as_str() {
            "random_walk" | "crash" => (g.drift, g.vol),
            "trend_up" => (g.drift.abs().max(0.002), g.vol.min(0.01)),
            "trend_down" => (-g.drift.abs().max(0.002), g.vol.min(0.01)),
            "sine" => (0.0, g.vol),
            other => return Err(format!("unknown generator `{other}` (random_walk, trend_up, trend_down, sine, crash)")),
        };
        Ok(Self { g: g.clone(), rng: Rng(g.seed ^ 0xA5A5_5A5A_1234_5678), close: g.start, i: 0, drift, vol, bar_secs })
    }

    /// Index of the bar `next_bar` will produce.
    pub fn position(&self) -> usize {
        self.i
    }

    pub fn next_bar(&mut self) -> Bar {
        let i = self.i;
        self.i += 1;
        let period = self.g.period.max(2) as f64;
        let open = self.close;
        self.close = if self.g.kind == "sine" {
            let base = self.g.start * (1.0 + self.g.amplitude * (2.0 * std::f64::consts::PI * i as f64 / period).sin());
            (base * (self.vol * self.rng.normal()).exp()).max(0.01)
        } else {
            let mut r = self.drift + self.vol * self.rng.normal();
            if self.g.kind == "crash" && i == self.g.bars * 6 / 10 {
                r -= 0.35;
            }
            open * r.exp()
        };
        let close = self.close;
        let wiggle = (self.vol / 3.0).max(0.0005);
        let high = open.max(close) * (1.0 + self.rng.uniform() * wiggle);
        let low = open.min(close) * (1.0 - self.rng.uniform() * wiggle);
        let volume = (1_000_000.0 * (1.0 + self.rng.normal().abs())).round();
        Bar { label: label(i, self.bar_secs), open, high, low, close, volume }
    }
}

pub fn generate(g: &Generator, bar_secs: f64) -> Result<Vec<Bar>, String> {
    if g.bars == 0 || g.bars > MAX_BARS {
        return Err(format!("bars must be between 1 and {MAX_BARS}"));
    }
    let mut stream = GenStream::new(g, bar_secs)?;
    Ok((0..g.bars).map(|_| stream.next_bar()).collect())
}

/// `YYYY-MM-DDTHH:MM:SSZ` for a Unix time in seconds.
pub fn iso_timestamp(unix_secs: i64) -> String {
    let days = unix_secs.div_euclid(86_400);
    let rem = unix_secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, (rem % 3600) / 60, rem % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_are_calendar_dates() {
        assert_eq!(label(0, 86_400.0), "2024-01-01");
        assert_eq!(label(31, 86_400.0), "2024-02-01");
        assert_eq!(label(60, 86_400.0), "2024-03-01"); // 2024 is a leap year
        assert_eq!(label(366, 86_400.0), "2025-01-01");
        assert_eq!(label(3, 300.0), "t+3");
    }

    #[test]
    fn csv_with_header_and_full_ohlc() {
        let bars = parse_csv("Date,Open,High,Low,Close,Volume\n2024-01-02,10,11,9,10.5,1000\n2024-01-03,10.5,12,10,11,2000\n", 86_400.0).unwrap();
        assert_eq!(bars.len(), 2);
        assert_eq!((bars[0].label.as_str(), bars[0].open, bars[0].close, bars[1].volume), ("2024-01-02", 10.0, 10.5, 2000.0));
    }

    #[test]
    fn csv_close_only_and_date_close() {
        let bars = parse_csv("close\n10\n11\n12\n", 86_400.0).unwrap();
        assert_eq!(bars.iter().map(|b| b.close).collect::<Vec<_>>(), [10.0, 11.0, 12.0]);
        assert_eq!(bars[1].label, "2024-01-02");
        let bars = parse_csv("2024-05-01, 5\n2024-05-02, 6\n", 86_400.0).unwrap();
        assert_eq!((bars[1].label.as_str(), bars[1].close), ("2024-05-02", 6.0));
        let bars = parse_csv("# comment\n\n7\n8\n", 86_400.0).unwrap();
        assert_eq!(bars.len(), 2);
    }

    #[test]
    fn csv_errors_name_the_line() {
        assert!(parse_csv("", 1.0).unwrap_err().contains("no data"));
        assert!(parse_csv("1\nabc\n", 1.0).unwrap_err().contains("line 2"));
        assert!(parse_csv("a,b,c\n1,2,3\n", 1.0).unwrap_err().contains("expected 1, 2 or 6"));
    }

    #[test]
    fn generators_are_deterministic_and_sane() {
        let g = Generator { bars: 100, ..Default::default() };
        let a = generate(&g, 86_400.0).unwrap();
        let b = generate(&g, 86_400.0).unwrap();
        assert_eq!(a, b);
        let c = generate(&Generator { seed: 2, ..g.clone() }, 86_400.0).unwrap();
        assert_ne!(a, c);
        assert!(a.iter().all(|x| x.close > 0.0 && x.high >= x.low && x.high >= x.open.min(x.close)));
        let up = generate(&Generator { kind: "trend_up".into(), ..g.clone() }, 86_400.0).unwrap();
        assert!(up.last().unwrap().close > up[0].close);
        let down = generate(&Generator { kind: "trend_down".into(), ..g.clone() }, 86_400.0).unwrap();
        assert!(down.last().unwrap().close < down[0].close);
        let crash = generate(&Generator { kind: "crash".into(), vol: 0.0, drift: 0.0, ..g.clone() }, 86_400.0).unwrap();
        assert!(crash[70].close < crash[50].close * 0.8);
    }

    #[test]
    fn generator_output_is_pinned() {
        // Pinned so that refactoring the generator cannot silently change saved seeds.
        let b = generate(&Generator { bars: 5, seed: 8, ..Default::default() }, 86_400.0).unwrap();
        let closes: Vec<String> = b.iter().map(|x| format!("{:.6}", x.close)).collect();
        assert_eq!(closes, ["100.907728", "101.415005", "100.643589", "102.287783", "102.486394"]);
        assert_eq!(b.iter().map(|x| x.volume).collect::<Vec<_>>(), [2291267.0, 1094150.0, 1264833.0, 1660114.0, 1398038.0]);
        assert_eq!((format!("{:.6}", b[2].high), format!("{:.6}", b[2].low)), ("101.629964".to_string(), "100.569010".to_string()));
        let s = generate(&Generator { kind: "sine".into(), bars: 5, seed: 3, ..Default::default() }, 86_400.0).unwrap();
        let closes: Vec<String> = s.iter().map(|x| format!("{:.6}", x.close)).collect();
        assert_eq!(closes, ["98.765073", "103.787780", "103.124886", "105.465261", "110.365729"]);
    }

    #[test]
    fn stream_matches_generate_and_can_run_past_the_requested_bars() {
        let g = Generator { bars: 50, seed: 4, ..Default::default() };
        let all = generate(&g, 86_400.0).unwrap();
        let mut s = GenStream::new(&g, 86_400.0).unwrap();
        let streamed: Vec<Bar> = (0..50).map(|_| s.next_bar()).collect();
        assert_eq!(all, streamed);
        assert_eq!(s.position(), 50);
        let extra = s.next_bar();
        assert!(extra.close > 0.0 && extra.label != streamed[49].label);
    }

    #[test]
    fn iso_timestamps() {
        assert_eq!(iso_timestamp(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso_timestamp(1_704_067_200 + 3_661), "2024-01-01T01:01:01Z");
        assert_eq!(iso_timestamp(1_709_164_800), "2024-02-29T00:00:00Z");
    }

    #[test]
    fn generator_rejects_bad_input() {
        assert!(generate(&Generator { bars: 0, ..Default::default() }, 1.0).is_err());
        assert!(generate(&Generator { kind: "nope".into(), ..Default::default() }, 1.0).is_err());
        assert!(generate(&Generator { start: -1.0, ..Default::default() }, 1.0).is_err());
        assert!(generate(&Generator { bars: MAX_BARS + 1, ..Default::default() }, 1.0).is_err());
    }

    #[test]
    fn market_validation() {
        let m = Market::single("X", bars_from_closes(&[1.0, 2.0], 86_400.0), 86_400.0);
        assert!(m.validate().is_ok());
        let mut bad = m.clone();
        bad.series.insert("Y".into(), bars_from_closes(&[1.0], 86_400.0));
        assert!(bad.validate().unwrap_err().contains("bars"));
        let zero = Market::single("X", bars_from_closes(&[1.0, 0.0], 86_400.0), 86_400.0);
        assert!(zero.validate().unwrap_err().contains("close"));
    }
}
