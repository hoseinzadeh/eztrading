use std::{collections::HashSet, fmt::{Display, Debug}};
use serde::{Serialize, Deserialize};

/// The interval in the time series. The following intervals are supported:
/// 1min, 5min, 15min, 30min, 45min, 1h, 2h, 4h, 1day, 1week, and 1month.
#[derive(PartialEq,Hash,Eq,Clone,Copy,Serialize,Deserialize)]
pub enum Interval {
    Every1Minute,
    Every5Minutes,
    Every15Minutes,
    Every30Minutes,
    Every45Minutes,
    Every1Hour,
    Every2Hours,
    Every4Hours,
    Daily,
    Weekly,
    Monthly
}

impl Display for Interval {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", match self {
            Interval::Every1Minute   => "Every Minute",
            Interval::Every5Minutes  => "Every 5 Minutes",
            Interval::Every15Minutes => "Every 15 Minutes",
            Interval::Every30Minutes => "Every Half an Hour",
            Interval::Every45Minutes => "Every 3 Quarters",
            Interval::Every1Hour     => "Hourly",
            Interval::Every2Hours    => "Every 2 Hours",
            Interval::Every4Hours    => "Every 4 Hours",
            Interval::Daily          => "Daily",
            Interval::Weekly         => "Weekly",
            Interval::Monthly        => "Monthly",
        })
    }
}

impl Debug for Interval {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self)
    }
}

impl Interval {
    #[inline]
    pub fn as_secs(&self) -> i64 {
        match self {
            Interval::Every1Minute   => 60,
            Interval::Every5Minutes  => 5 * 60,
            Interval::Every15Minutes => 15 * 60,
            Interval::Every30Minutes => 30 * 60,
            Interval::Every45Minutes => 45 * 60,
            Interval::Every1Hour     => 3600,
            Interval::Every2Hours    => 2 * 3600,
            Interval::Every4Hours    => 4 * 3600,
            Interval::Daily          => 24 * 3600,
            Interval::Weekly         => 7 * 24 * 3600,
            Interval::Monthly        => 30 * 7 * 24 * 3600,
        }
    }

    pub fn from_str(n: &str) -> Option<Self> {
        use Interval::*;
        match n {
            "1min" => Some(Every1Minute),
            "5min" => Some(Every5Minutes),
            "15min" => Some(Every15Minutes),
            "30min" => Some(Every30Minutes),
            "45min" => Some(Every45Minutes),
            "1h" => Some(Every1Hour),
            "2h" => Some(Every2Hours),
            "4h" => Some(Every4Hours),
            "daily" => Some(Daily),
            "weekly" => Some(Weekly),
            "monthly" => Some(Monthly),
            _ => None
        }
    }
}

/// This is to be used to create a search filter can be used for quoting,
/// searching, and time series.
/// 
/// # Example
///
/// ```
/// use com::data::search::Filter;
///
/// let symbols = vec!["AAPL".to_string(), "GOOG".to_string(), "MSFT".to_string()];
/// let filter = Filter::new()
///     .symbols(&symbols)
///     .country("US".to_string())
///     .exchange("NASDAQ".to_string());
/// assert_eq!(filter.symbols.len(), 3);
/// assert_eq!(filter.exchange.as_deref(), Some("NASDAQ"));
/// ```
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Filter {
    pub start: Option<i64>,
    pub end: Option<i64>,
    pub symbols: HashSet<String>,
    pub country: Option<String>,
    pub exchange: Option<String>,
    pub mic: Option<String>,
    pub instrument_type: Option<String>,
    pub interval: Option<Interval>,
}

impl Filter {
    pub fn new() -> Self {
        Self {
            start: None,
            end: None,
            symbols: HashSet::new(),
            country: None,
            exchange: None,
            mic: None,
            instrument_type: None,
            interval: None,
        }
    }

    pub fn start(mut self, date: i64) -> Self {
        self.start = Some(date);
        self
    }

    pub fn end(mut self, date: i64) -> Self {
        self.end = Some(date);
        self
    }

    pub fn symbol(mut self, symbol: String) -> Self {
        self.symbols.insert(symbol);
        self
    }

    pub fn exchange(mut self, exchange: String) -> Self {
        self.exchange = Some(exchange);
        self
    }

    pub fn mic(mut self, mic: String) -> Self {
        self.mic = Some(mic);
        self
    }

    pub fn country(mut self, country: String) -> Self {
        self.country = Some(country);
        self
    }

    pub fn symbols(mut self, symbols: &Vec<String>) -> Self {
        let set: HashSet<String> = self.symbols.union(
                &HashSet::from_iter(symbols.iter().cloned()))
                .map(|x| x.to_owned()).collect();
        self.symbols = set;
        self
    }

    pub fn interval(mut self, int: Interval) -> Self {
        self.interval = Some(int);
        self
    }
}