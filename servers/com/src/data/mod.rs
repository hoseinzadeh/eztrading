use chrono::{Datelike, NaiveDateTime, Timelike};
use serde::{Deserialize, Serialize};
use std::fmt::Debug;

mod cvec;
pub mod series;
mod param;
pub mod market;
pub mod provider;
pub mod search;
pub use cvec::*;
pub use param::*;

use self::search::Interval;

#[derive(Copy, Clone, Serialize, Deserialize)]
#[repr(C)]
pub struct Ticker {
    pub datetime: i64,
    pub open: f32,
    pub high: f32,
    pub low: f32,
    pub close: f32,
    pub volume: i64,
    pub time_flags: u32,
}

impl Debug for Ticker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let datetime = NaiveDateTime::from_timestamp_opt(self.datetime as i64, 0)
            .map(|t| t.to_string())
            .unwrap_or("unknown".into());
        f.debug_struct("")
            .field("datetime", &datetime)
            .field("open", &self.open)
            .field("high", &self.high)
            .field("low", &self.low)
            .field("close", &self.close)
            .field("volume", &self.volume)
            .finish()
    }
}

impl PartialEq for Ticker {
    fn eq(&self, other: &Self) -> bool {
        self.datetime == other.datetime
    }
}

impl PartialOrd for Ticker {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.datetime.partial_cmp(&other.datetime)
    }
}

impl Ord for Ticker {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.datetime.cmp(&other.datetime)
    }
}

impl PartialOrd<i64> for Ticker {
    fn partial_cmp(&self, other: &i64) -> Option<std::cmp::Ordering> {
        self.datetime.partial_cmp(other)
    }
}

impl PartialEq<i64> for Ticker {
    fn eq(&self, other: &i64) -> bool {
        self.datetime == *other
    }
}

impl Eq for Ticker {}

impl Ticker {
    pub fn flags(datetime: NaiveDateTime) -> u32 {
        use crate::data::*;
        use chrono::Weekday;

        let mut res = 0;
        if datetime.minute() == 59 {
            res = IS_EOH;
            if datetime.hour() == 15 {
                // TODO: Change it to Market::CLOSE_HOUR
                res |= IS_EOD;
                if datetime.weekday() == Weekday::Fri {
                    res |= IS_EOW
                }
                if datetime.day() == Self::days_in_month(datetime.year(), datetime.month()) {
                    res |= IS_EOM
                }
            }
        }
        res
    }

    pub fn days_in_month(year: i32, month: u32) -> u32 {
        use chrono::{Duration, NaiveDate};

        // the first day of the next month...
        let (y, m) = if month == 12 {
            (year + 1, 1)
        } else {
            (year, month + 1)
        };
        let d = NaiveDate::from_ymd_opt(y, m, 1).unwrap() - Duration::days(1);
        d.day()
    }
}

pub const IS_EOH: u32 = 0x1;
pub const IS_EOD: u32 = 0x2;
pub const IS_EOW: u32 = 0x4;
pub const IS_EOM: u32 = 0x8;

pub trait Timed {
    fn time(&self) -> i64;
    fn update(&mut self, val: &Self, int: &Interval);
    fn flags(&self) -> u32;

    /// Is End of Hour
    #[inline]
    fn is_eoh(&self) -> bool {
        self.flags() & IS_EOH != 0
    }

    /// Is End of Day
    #[inline]
    fn is_eod(&self) -> bool {
        self.flags() & IS_EOD != 0
    }

    /// Is End of Week
    #[inline]
    fn is_eow(&self) -> bool {
        self.flags() & IS_EOW != 0
    }

    /// Is End of Month
    #[inline]
    fn is_eom(&self) -> bool {
        self.flags() & IS_EOM != 0
    }
}

impl Timed for Ticker {
    fn time(&self) -> i64 {
        self.datetime
    }

    fn update(&mut self, val: &Self, int: &Interval) {
        let t = int.as_secs();
        self.datetime = (val.datetime / t) * t;
        self.close = val.close;
        self.low = self.low.min(val.close);
        self.high = self.high.max(val.close);
        self.volume = val.volume;
    }

    #[inline]
    fn flags(&self) -> u32 {
        self.time_flags
    }
}

pub trait PriceFeed {
    /// Adjust the volume given the previous day_volume from the websocket
    fn adjust(&self, last_day_volume: &mut i64) -> Self;
    fn reset(self) -> Self;
    fn volume(&self) -> i64;
    fn open(&self) -> f32;
    fn close(&self) -> f32;
}

impl PriceFeed for Ticker {
    fn adjust(&self, last_day_volume: &mut i64) -> Self {
        let mut res = self.clone();
        if res.open == 0.0 {
            res.open = res.close;
        }
        if *last_day_volume == 0 {
            // The first feed update may mess the volume value because the last
            // value from the list is not a day_value and therefore it is smaller
            // than the feed's one. So, we skip this one and update the
            // `last_day_volume`, so that the next call to `adjust` works fine.
            *last_day_volume = res.volume;

            res.volume = 0;
        } else if *last_day_volume <= res.volume {
            res.volume -= *last_day_volume;
        } else {
            res.volume = 0;
        }
        res
    }

    fn reset(mut self) -> Self {
        self.open = self.close;
        self.low = self.close;
        self.high = self.close;
        self.volume = 0;
        self
    }

    fn volume(&self) -> i64 {
        self.volume
    }

    fn open(&self) -> f32 {
        self.open
    }

    fn close(&self) -> f32 {
        self.close
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TimeSeriesMetaData {
    pub symbol: String,
    pub interval: String,
    pub currency: Option<String>,
    pub exchange_timezone: Option<String>,
    pub currency_base: Option<String>,
    pub currency_quote: Option<String>,
    pub exchange: String,
    pub mic_code: Option<String>,
    pub r#type: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TimeSeries {
    pub meta: TimeSeriesMetaData,
    pub values: Vec<Ticker>,
}

impl TimeSeries {
    pub fn values(&self) -> &Vec<Ticker> {
        &self.values
    }
}