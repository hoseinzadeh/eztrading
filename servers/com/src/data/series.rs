use super::{
    provider::Provider,
    search::{Filter, Interval},
    CVec, Ticker, Timed,
};
use crate::status::Status;
use chrono::{Utc, NaiveDateTime};
use std::{collections::HashMap, fmt::Debug};

/*
Initial Steps:
    0. Q_0   <- WebSocket (live data)
    1. Q_1m  <- Fetch 1 min time series
    2. Q_5m  <- Fetch 5 min time series
    3. Q_15m <- Fetch 15 min time series
    .
    .
    .
    4. Q_1day <- Fetch daily time series
    5. Q <- Q_1m[..Q_0.first.timestamp]
    6. Q <- push front Q_5m[..Q_1m.first.timestamp]
    7. Q <- push front Q_15m[..Q_5m.first.timestamp]
    .
    .
    .
    8. Q <- push back Q_0
    9. Switch WebSocket to update Q
    10. P <- Convert Q to Shared Vector
    11. Send P to all JobSrvs
    12. D <- Q-P (If there is any new updates arrived through the WS)
    13. if D != 0 then copy new data to P
    14. Switch WS to update P
    15. Send the updated segment to JobSrvs
    16. On each data arrival, send it to all JobSrvs

Reload:
    0. Q_0  <- WebSocket (live data)
    1. P <- Load from file
    2. D <- Q_0.first - P.last
    3. if D < 1min then goto init[12]
    4. if D < 5min then
        4.1. Q_1m <- Fetch 1 min time series from D.last
        4.2. P <- push back Q_1m[0..Q_0.first]
        4.3. P <- push back Q_0
        4.4. goto 14
 */

/**
Represent a data stream

This struct can be used to fetch initial data from the server and attach it to a
[`CVec`] object to store updates on a file. [`DataSrv`] sends the init file to
each [`JobSrv`]. Afterwards, each new call to the [`update`] function
distributes data directly between the [`JobSrv`]s, and the stream file, as well.

### Consistency
When the [`CVar`] is constructible from the file, it indicates that there was a
crash/restart. In this case, [`Series`] loads data, calculates the time distance
between the last value and the current time, fetches only the gapped time
series, and reconstruct the history. Then, it sends the new history file to the
[`JobSrv`]s and listens to the websocket for new data.
*/
#[derive(Debug)]
pub struct Series {
    data: HashMap<Interval, (CVec<Ticker>, i64)>,
    symbol: String,
    exchange: Option<String>,
}

impl Series {
    /// Returns the size of vector stored in a file in bytes
    #[inline]
    pub fn size_of(cap: usize) -> usize {
        11 * CVec::<Ticker>::size_of(cap)
    }

    /// Returns a new instance of [`Series`] and the new file position
    ///
    /// * `base`: the base address of the memory mapped file
    /// * `offset`: position of this object in the file
    /// * `trunc`: truncate the memory for all vectors
    /// * `cap`: the maximum number of objects on each vector
    pub fn new<S: ToString>(
        symbol: S,
        mut base: *mut u8,
        trunc: bool,
        cap: usize,
    ) -> (Self, *mut u8) {
        use Interval::*;
        static INTERVALS: [Interval; 11] = [
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
            Monthly,
        ];
        let mut data = HashMap::new();
        for int in &INTERVALS {
            unsafe {
                let cvec = CVec::<Ticker>::new(base, trunc, cap);
                base = cvec.next_base();
                data.insert(*int, (cvec, 0));
            }
        }
        (
            Self {
                symbol: symbol.to_string(),
                data,
                exchange: None,
            },
            base,
        )
    }

    pub fn fetch<P: Provider>(
        &mut self,
        filter: &Filter,
        provider: &mut P
    ) -> Result<Vec<Ticker>, Status> {
        let mut opening = None;

        // If market is None, it means it is 24/7
        if let Some(exch) = &self.exchange {
            if let Some(market) = provider.markets().get(exch) {
                if !market.is_open() {
                    opening = market.opening_time();
                }
            }
        }

        let int = filter.interval.unwrap_or(Interval::Every1Minute);
        let (vec, last_fetch_time) = self.data.get_mut(&int).unwrap();
        if let Some(last) = vec.last() {
            // Fetch missing data
            let cur_time = Utc::now().timestamp();
            let mut diff = cur_time - last.time();

            // If the diff is more than 5 days, fetch from the last time
            if let Some(p) = opening {
                if cur_time > p {
                    diff = cur_time - p;
                }
            }
            // Use half interval to cover delayed data
            let last_time_threshold = *last_fetch_time + int.as_secs() / 2;
            if diff > int.as_secs() + 30 && cur_time > last_time_threshold {
                let mut filter = filter.clone().start(last.time() + 1).end(cur_time + 360);
                if let Some(exch) = &self.exchange {
                    filter = filter.exchange(exch.clone());
                }
                filter.symbols = vec![self.symbol.clone()].into_iter().collect();
                match provider.retrieve_history(filter) {
                    Ok(data) => if let Some(new) = data.get(&self.symbol) {
                        self.exchange = Some(new.meta.exchange.clone());
                        vec.append(&new.values);
                    }
                    Err(e) => eprintln!("{e}")
                }
                *last_fetch_time = cur_time;
            } else {
                println!("Last time fetched at {}, try again at {}.",
                    NaiveDateTime::from_timestamp_opt(*last_fetch_time, 0).unwrap(),
                    NaiveDateTime::from_timestamp_opt(last_time_threshold, 0).unwrap(),
                )
            }
        } else {
            // Fetch all data
            let mut filter = Filter::new()
                .symbol(self.symbol.clone())
                .start(-100000)
                .interval(int);
            if let Some(exch) = &self.exchange {
                filter = filter.exchange(exch.clone());
            }
            if let Some(new) = provider.retrieve_history(filter)?.get(&self.symbol) {
                self.exchange = Some(new.meta.exchange.clone());
                vec.append(&new.values)
            }
        };

        let mut res = vec![];
        match (filter.start, filter.end) {
            (None, None) => {
                if let Some(v) = self.data[&int].0.last() {
                    res.push(v.clone())
                }
            }
            (lo, hi) => res = unsafe { self.data[&int].0.subset(lo, hi) },
        }
        Ok(res)
    }
}
