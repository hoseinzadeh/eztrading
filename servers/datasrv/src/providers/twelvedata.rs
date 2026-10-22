//! This Module provides an interface to collect stock market data from
//! twelvedata.com through API and WebSockets for live data.

use chrono::{NaiveDateTime, Utc};
use colored::*;
use com::{
    convert,
    data::{
        market::Market,
        provider::{Provider, HISTORY_SIZE},
        search::{Filter, Interval},
        series::Series,
        Ticker, TimeSeries, TimeSeriesMetaData,
    },
    failed,
    net::*,
    status::{ErrorCode, Status},
};
use crossbeam_channel::{RecvTimeoutError, Sender};
use serde::{Deserialize, Serialize};
use serde_json::{self, Value};
use std::net::SocketAddr;
use std::sync::atomic::AtomicI32;
use std::{
    collections::HashMap,
    fmt::Debug,
    sync::{
        atomic::{AtomicBool, Ordering::Relaxed},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

const LENGTH_LIMIT: i64 = 5000;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TwelveDataTimeSeries(HashMap<String, TSTicker>);

impl From<TwelveDataTimeSeries> for HashMap<String, TimeSeries> {
    fn from(value: TwelveDataTimeSeries) -> Self {
        value
            .0
            .into_iter()
            .map(|(k, v)| {
                (
                    k,
                    TimeSeries {
                        meta: v.meta.unwrap().into(),
                        values: v
                            .values
                            .unwrap()
                            .iter()
                            .rev()
                            .map(|v| Ticker::from(v))
                            .collect(),
                    },
                )
            })
            .collect()
    }
}

/// TimeSeries point value
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TSPoint {
    datetime: String,
    high: String,
    low: String,
    close: String,
    open: String,
    volume: Option<String>,
}

impl From<&TSPoint> for Ticker {
    fn from(value: &TSPoint) -> Self {
        let mut datetime = value.datetime.to_owned();
        if value.datetime.len() < 16 {
            // TODO: What time should be added?
            datetime = format!("{} 00:00:00", value.datetime);
        }

        let datetime = NaiveDateTime::parse_from_str(&datetime, "%Y-%m-%d %H:%M:%S")
            .expect("Invalid Date Time format");

        Self {
            datetime: datetime.timestamp(),
            open: value.open.parse().expect("Invalid open value"),
            high: value.high.parse().expect("Invalid high value"),
            low: value.low.parse().expect("Invalid low value"),
            close: value.close.parse().expect("Invalid close value"),
            volume: value
                .volume
                .as_ref()
                .map(|a| a.parse().unwrap_or(0))
                .unwrap_or(0),
            time_flags: Ticker::flags(datetime),
        }
    }
}

/// Base URL of the Twelve Data REST API (`TWELVEDATA_API_URL` overrides it, e.g. for a local stand-in).
fn api_url() -> String {
    std::env::var("TWELVEDATA_API_URL").unwrap_or_else(|_| "https://api.twelvedata.com".into()).trim_end_matches('/').to_owned()
}

/// Base URL of the Twelve Data WebSocket API (`TWELVEDATA_WS_URL` overrides it).
fn ws_url() -> String {
    std::env::var("TWELVEDATA_WS_URL").unwrap_or_else(|_| "wss://ws.twelvedata.com".into()).trim_end_matches('/').to_owned()
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TSTicker {
    meta: Option<TimeSeriesMetaData>,
    values: Option<Vec<TSPoint>>,
    message: Option<String>,
    status: String,
}

/// WebSocket price response
#[derive(Clone, Debug, Deserialize, Serialize, Default)]
pub struct WSValue {
    event: String,
    symbol: Option<String>,
    currency: Option<String>,
    currency_base: Option<String>,
    currency_quote: Option<String>,
    exchange: Option<String>,
    r#type: Option<String>,
    timestamp: Option<i64>,
    price: Option<f32>,
    bid: Option<f32>,
    ask: Option<f32>,
    day_volume: Option<i64>,
    mic_code: Option<String>,
}

impl From<WSValue> for Ticker {
    fn from(value: WSValue) -> Self {
        Self {
            datetime: value.timestamp.unwrap(),
            open: 0.0,
            high: 0.0,
            low: 0.0,
            close: value.price.unwrap(),
            volume: value.day_volume.unwrap_or(0),
            time_flags: 0,
        }
    }
}

pub struct WS12Data {
    hb_received: Arc<AtomicBool>,
    is_connected: Arc<AtomicBool>,
}

impl WebSocketClient for WS12Data {
    fn heartbeat(&self) -> &Arc<AtomicBool> {
        &self.hb_received
    }

    fn connected(&self) -> &Arc<AtomicBool> {
        &self.is_connected
    }

    fn create(
        symbols: Arc<Mutex<Vec<String>>>,
        stream_tx: Sender<(String, Ticker)>,
        key: &str,
    ) -> Result<(), Status> {
        let url = format!("{}/v1/quotes/price?apikey={key}", ws_url());
        let symbols = symbols.lock().unwrap().join(",");
        let socket = Self::connect(&url)?;
        let ws = Self {
            hb_received: Arc::new(AtomicBool::new(true)),
            is_connected: Arc::new(AtomicBool::new(true)),
        };

        let (thrd, tx, rx) = ws.spawn(socket, stream_tx);

        let hb = ws.hb_received.clone();
        let is_connected = ws.is_connected.clone();
        let reconnect_msg = format!(
            "{{
            \"action\": \"subscribe\",
            \"params\": {{
            \"symbols\": \"{}\"
            }}
        }}",
            symbols
        );
        let tx_clone = tx.clone();
        let reconnect_msg_clone = reconnect_msg.clone();
        let builder = thread::Builder::new().name("websocket-receiver".into());
        let thrd2 = builder
            .spawn(move || {
                while is_connected.load(Relaxed) {
                    match rx.recv_timeout(Duration::from_secs(5)) {
                        Ok(WSMessage::Text(msg)) => {
                            if let Ok(value) = serde_json::from_str::<WSValue>(msg.as_str()) {
                                if value.event == "heartbeat" {
                                    hb.store(true, Relaxed);
                                } else if value.event == "price" {
                                    let t: Ticker = value.clone().into();
                                    if let Err(e) =
                                        tx.send(WSConnMessage::Ticker(value.symbol.unwrap(), t))
                                    {
                                        return Err(failed!(e, ErrorCode::Network));
                                    }
                                }
                            } else {
                                println!("Received: {}", msg);
                            }
                        }
                        Ok(WSMessage::Heartbeat) => {
                            if let Err(e) =
                                tx.send(WSConnMessage::Text(r#"{"action": "heartbeat"}"#.into()))
                            {
                                return Err(failed!(e, ErrorCode::Network));
                            }
                        }
                        Ok(WSMessage::Retry) => {
                            if let Err(e) = tx.send(WSConnMessage::Text(reconnect_msg.clone())) {
                                return Err(failed!(e, ErrorCode::Network));
                            }
                        }
                        Ok(WSMessage::Reconnect) => {
                            if let Err(e) = tx.send(WSConnMessage::Reconnect(url.clone())) {
                                return Err(failed!(e, ErrorCode::Network));
                            }
                        }
                        Ok(WSMessage::Disconnect) => {
                            is_connected.store(false, Relaxed);
                            return Err(failed!("Connection lost", ErrorCode::Network));
                        }
                        Err(RecvTimeoutError::Disconnected) => {
                            eprintln!("WS Channel: disconnected")
                        }
                        Err(_) => (),
                    }
                }
                Ok(Status::successful())
            })
            .unwrap();

        convert!(
            tx_clone.send(WSConnMessage::Text(reconnect_msg_clone)),
            ErrorCode::Network
        )?;

        println!(
            "{}",
            format!("Real-time streaming started for {symbols}.").blue()
        );

        for t in [thrd, thrd2] {
            if let Err(e) = t.join().unwrap() {
                eprintln!("{e}")
            }
        }

        Ok(())
    }
}

#[derive(Default)]
pub struct TwelveData {
    workspace: String,
    symbols: Arc<Mutex<Vec<String>>>,
    thread_handles: Arc<Mutex<Vec<JoinHandle<Status>>>>,
    streams: Arc<HashMap<String, Mutex<Series>>>,
    working: Arc<AtomicBool>,
    markets: Arc<HashMap<String, Market<Self>>>,
    apikey: String,
    ws_price_channels: Arc<Mutex<HashMap<String, Vec<Sender<WSPrice>>>>>,
    listeners: Arc<Mutex<HashMap<Symbol, Vec<Sender<Packet>>>>>,
}

fn default_atomic_bool() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(true))
}

impl TwelveData {
    /// Convert an [`Interval`] to string for building the time-series query
    fn interval_to_str(int: &Interval) -> &str {
        match int {
            Interval::Every1Minute => "1min",
            Interval::Every5Minutes => "5min",
            Interval::Every15Minutes => "15min",
            Interval::Every30Minutes => "30min",
            Interval::Every45Minutes => "45min",
            Interval::Every1Hour => "1h",
            Interval::Every2Hours => "2h",
            Interval::Every4Hours => "4h",
            Interval::Daily => "1day",
            Interval::Weekly => "1week",
            Interval::Monthly => "1month",
        }
    }

    /// Convert a timestamp to string for building the time-series query    
    fn timestamp_to_str(t: i64) -> String {
        NaiveDateTime::from_timestamp_opt(t as i64, 0)
            .unwrap()
            .to_string()
    }

    fn retrieve_time_series(
        filter: Filter,
        key: &str,
    ) -> Result<HashMap<String, TimeSeries>, Status> {
        let request_url = format!(
                "{api}/time_series?apikey={key}&symbol={symbols}&timezone=UTC{exchange}{mic}{country}{start}{end}{interval}",
                api = api_url(),
                symbols = filter.symbols.iter().map(|s| s.to_string()).collect::<Vec<String>>().join(","),
                exchange = to_url_query("exchange", &filter.exchange),
                mic = to_url_query("mic_code", &filter.mic),
                country = to_url_query("country", &filter.country),
                start = filter.start.map(|v| format!("&start_date={}", Self::timestamp_to_str(v))).unwrap_or(String::new()),
                end = filter.end.map(|v| format!("&end_date={}", Self::timestamp_to_str(v))).unwrap_or(String::new()),
                interval = if let Some(int) = &filter.interval { format!("&interval={}", Self::interval_to_str(int)) } else { "".to_owned() },
            );
        println!(
            "Fetching {} time-series for {} from {} to {} ...",
            if let Some(int) = &filter.interval {
                format!("{:?}", int).yellow()
            } else {
                "".yellow()
            },
            format!(
                "{:?}",
                filter
                    .symbols
                    .iter()
                    .map(|s| s.to_string())
                    .collect::<Vec<String>>()
                    .join(",")
            )
            .green(),
            if let Some(date) = filter.start {
                Self::timestamp_to_str(date).blue()
            } else {
                "".blue()
            },
            if let Some(date) = filter.end {
                Self::timestamp_to_str(date).blue()
            } else {
                "present".blue()
            },
        );

        let response = convert!(reqwest::blocking::get(&request_url), ErrorCode::Network)?;
        let txt = response.text();

        if filter.symbols.len() == 1 {
            let time_series: TSTicker =
                convert!(serde_json::from_str(txt.unwrap().as_str()), ErrorCode::Data)?;
            if let Some(msg) = time_series.message {
                Err(failed!(
                    format!("{}\nQuery: {}", msg.red(), request_url.truecolor(128, 0, 0)),
                    ErrorCode::Data
                ))
            } else {
                let mut res = TwelveDataTimeSeries(HashMap::new());
                let sym = filter.symbols.iter().last().unwrap();
                println!(
                    "{sym} actual count = {}",
                    time_series.values.as_ref().unwrap().len()
                );
                res.0.insert(sym.clone(), time_series);
                Ok(res.into())
            }
        } else {
            let time_series: TwelveDataTimeSeries =
                convert!(serde_json::from_str(txt.unwrap().as_str()), ErrorCode::Data)?;
            let mut errs = vec![];
            for (sym, val) in &time_series.0 {
                if let Some(msg) = &val.message {
                    errs.push(failed!(format!("{sym}: {}", msg.red()), ErrorCode::Data))
                } else {
                    println!(
                        "{sym} actual count = {}",
                        val.values.as_ref().unwrap().len()
                    );
                }
            }
            if errs.is_empty() {
                Ok(time_series.into())
            } else {
                let mut err_lst = vec![failed!(format!(
                    "Errors on Query: {}",
                    request_url.truecolor(128, 0, 0)
                ))];
                err_lst.append(&mut errs);
                Err(Status::Multi(err_lst))
            }
        }
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct WSPrice {
    price: f32,
}

impl From<Ticker> for WSPrice {
    fn from(value: Ticker) -> Self {
        Self { price: value.close }
    }
}

impl Provider for TwelveData {
    type WS = WS12Data;
    type WSData = WSPrice;

    #[inline]
    fn workspace(&self) -> &str {
        &self.workspace
    }

    #[inline]
    fn symbols(&self) -> &Arc<Mutex<Vec<String>>> {
        &self.symbols
    }

    #[inline]
    fn streams(&self) -> &Arc<HashMap<String, Mutex<Series>>> {
        &self.streams
    }

    fn set_streams(&mut self, streams: Arc<HashMap<String, Mutex<Series>>>) {
        self.streams = streams;
    }

    fn init(&mut self, key: &str) -> Result<(), Status> {
        let request_url = format!("{}/market_state?apikey={key}", api_url());
        let response = convert!(reqwest::blocking::get(request_url), ErrorCode::Network)?;
        let txt = response.text();
        let json: serde_json::Value =
            convert!(serde_json::from_str(txt.unwrap().as_str()), ErrorCode::Data)?;
        self.apikey = key.to_owned();

        fn extract(v: &Value, name: &str) -> Result<String, Status> {
            if let Some(val) = v[name].as_str() {
                Ok(val.to_owned())
            } else {
                Err(failed!(format!("No value found for {name}")))
            }
        }

        fn extract_i64(v: &Value, name: &str) -> Result<i64, Status> {
            if let Some(val) = v[name].as_str() {
                let parts: Vec<i64> = val
                    .split(":")
                    .map(|v| convert!(v.parse::<i64>()).unwrap())
                    .collect();
                Ok(parts[0] * 3600 + parts[1] * 60 + parts[2])
            } else {
                Err(failed!(format!("No value found for {name}")))
            }
        }

        fn init_market<P: Provider>(
            m: &mut Market<P>,
            now: i64,
            time_to_close: i64,
            time_to_open: i64,
        ) {
            if m.is_open() {
                m.opening_time = None;
                m.closing_time = Some(now + time_to_close);
            } else {
                m.opening_time = Some(now + time_to_open);
                m.closing_time = None;
            }
        }

        let mut exchange_markets = HashMap::new();
        if let Some(markets) = json.as_array() {
            let now = Utc::now().timestamp();
            for market in markets {
                let mut m: Market<Self> = Market::new(
                    extract(market, "name")?,
                    extract(market, "code")?,
                    extract(market, "country")?,
                );
                m.set_open(market["is_market_open"].as_bool().unwrap());
                init_market(
                    &mut m,
                    now,
                    extract_i64(market, "time_to_close")?,
                    extract_i64(market, "time_to_open")?,
                );
                exchange_markets.insert(m.name().to_owned(), m);
            }
            self.markets = Arc::new(exchange_markets);
            Ok(())
        } else {
            Err(failed!("Could not retrieve market status"))
        }
    }

    fn retrieve_history(&mut self, filter: Filter) -> Result<HashMap<String, TimeSeries>, Status> {
        if filter.symbols.len() != 1 {
            Err(failed!("Only one symbol at a time"))
        } else {
            let sym = filter.symbols.iter().last().unwrap();
            let earliest_timestamps = self.earliest_timestamps(filter.clone())?;
            let int = filter.interval.unwrap_or(Interval::Daily).as_secs();
            let mut ets = *earliest_timestamps[sym].as_ref().unwrap();
            let now = Utc::now().timestamp();
            if let Some(start) = filter.start {
                ets = ets.max(start);
                ets = ets.max(now - HISTORY_SIZE as i64 * int)
            }
            println!(
                "Earliest timestamp is {}",
                NaiveDateTime::from_timestamp_opt(ets, 0).unwrap()
            );
            let mut count = (now - ets) / int;
            if count > LENGTH_LIMIT {
                let mut values = vec![];
                let mut meta = None;
                while ets < now {
                    let top = ets + LENGTH_LIMIT * int - 1;
                    let f = filter.clone().start(ets).end(top);
                    println!(
                        "Retrieving data from {} to {}",
                        NaiveDateTime::from_timestamp_opt(ets, 0).unwrap(),
                        NaiveDateTime::from_timestamp_opt(top, 0).unwrap()
                    );
                    if let Ok(mut ts) = Self::retrieve_time_series(f, &self.apikey) {
                        let ts = ts.get_mut(sym).unwrap();
                        if let Some(first) = ts.values.first() {
                            println!("First Data: {:#?}", first);
                        }
                        if let Some(last) = ts.values.last() {
                            println!("Last Data: {:#?}", last);
                        }
                        meta = Some(ts.meta.clone());
                        values.append(&mut ts.values);
                        ets = top;
                        count -= LENGTH_LIMIT;
                    } else {
                        break;
                    }
                }
                if let Some(first) = values.first() {
                    println!("Final First Data: {:#?}", first);
                }
                if let Some(last) = values.last() {
                    println!("Final Last Data: {:#?}", last);
                }

                let mut res = HashMap::new();
                res.insert(
                    sym.to_owned(),
                    TimeSeries {
                        meta: meta.unwrap(),
                        values,
                    },
                );
                Ok(res)
            } else {
                Self::retrieve_time_series(filter, &self.apikey)
            }
        }
    }

    fn earliest_timestamps(
        &mut self,
        filter: Filter,
    ) -> Result<HashMap<String, Result<i64, String>>, Status> {
        let request_url = format!(
            "{api}/earliest_timestamp?apikey={key}&symbol={symbols}&timezone=UTC{exchange}{mic}{interval}",
            key = self.apikey,
            api = api_url(),
            symbols = filter.symbols.iter().map(|s| s.to_string()).collect::<Vec<String>>().join(","),
            exchange = to_url_query("exchange", &filter.exchange),
            mic = to_url_query("mic_code", &filter.mic),
            interval = if let Some(int) = &filter.interval { format!("&interval={}", Self::interval_to_str(int)) } else { "".to_owned() },
        );
        let response = convert!(reqwest::blocking::get(&request_url), ErrorCode::Network)?;
        let txt = response.text();

        #[derive(Serialize, Deserialize)]
        struct EarliestTimeStamp {
            datetime: Option<String>,
            unix_time: Option<i64>,
            message: Option<String>,
            status: Option<String>,
            code: Option<i32>,
        }

        if let Ok(e) = serde_json::from_str::<EarliestTimeStamp>(txt.as_ref().unwrap().as_str()) {
            if let Some(msg) = e.message {
                return Err(failed!(
                    format!("{}\nQuery: {}", msg.red(), request_url.truecolor(128, 0, 0)),
                    ErrorCode::Data
                ));
            } else if filter.symbols.len() == 1 {
                let mut res = HashMap::new();
                let sym = filter.symbols.iter().last().unwrap();
                res.insert(sym.clone(), Ok(e.unix_time.unwrap()));
                return Ok(res);
            }
        }

        let earliest_time: HashMap<String, EarliestTimeStamp> =
            convert!(serde_json::from_str(txt.unwrap().as_str()), ErrorCode::Data)?;
        // let save_earliest_time: HashMap<String, Option<i64>> = earliest_time
        // .iter()
        // .map(|(k, v)| (k.to_owned(), v.unix_time))
        // .collect();

        // self.set_earliest_timestamps(filter.interval.unwrap(), save_earliest_time);

        Ok(earliest_time
            .into_iter()
            .map(|(k, v)| {
                (
                    k,
                    if let Some(msg) = v.message {
                        Err(msg)
                    } else {
                        Ok(v.unix_time.unwrap())
                    },
                )
            })
            .collect())
    }

    #[inline]
    fn is_working(&self) -> &Arc<AtomicBool> {
        &self.working
    }

    fn stop(&self) {
        self.working.store(false, Relaxed)
    }

    fn create<S1: ToString, S2: ToString>(workspace: S1, apikey: S2) -> Self {
        println!("{}: create", chrono::Local::now());
        Self {
            symbols: Arc::new(Mutex::new(vec![])),
            streams: Default::default(),
            workspace: workspace.to_string(),
            working: default_atomic_bool(),
            thread_handles: Arc::new(Mutex::new(vec![])),
            markets: Default::default(),
            apikey: apikey.to_string(),
            ws_price_channels: Arc::new(Mutex::new(HashMap::new())),
            listeners: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn threads(&self) -> &Arc<Mutex<Vec<JoinHandle<Status>>>> {
        &self.thread_handles
    }

    fn markets(&self) -> &Arc<HashMap<String, Market<Self>>> {
        &self.markets
    }

    fn start_websocket_service(
        &self,
        cert: &str,
        key: &str,
        addr: impl Into<SocketAddr>,
    ) -> Result<(), Status> {
        let mut ws = self.realtime_stream();
        ws.run(
            cert,
            key,
            addr,
            ws.recv.f.clone(),
            ws.init.f.clone(),
            ws.closed.f.clone(),
            ws.id.clone(),
        )?;
        Ok(())
    }

    fn websocket_channels(&self) -> &Arc<Mutex<HashMap<Symbol, Vec<Sender<Self::WSData>>>>> {
        &self.ws_price_channels
    }

    fn listeners(&self) -> &Arc<Mutex<HashMap<Symbol, Vec<Sender<Packet>>>>> {
        &self.listeners
    }

    fn send_to_listener(sym: &String, t: Ticker) -> Packet {
        Packet::from(JobSrvReq::Update(sym.clone(), t)).async_process()
    }

    fn set_workspace<S: ToString>(&mut self, path: S) {
        self.workspace = path.to_string()
    }
}

#[derive(Clone)]
struct Helper<F> {
    f: Arc<F>,
}

#[derive(Clone)]
pub struct TwelveDataWebSocket<RecvFn, InitFn, ClosedFn> {
    ws_is_open: Arc<AtomicBool>,
    recv: Helper<RecvFn>,
    init: Helper<InitFn>,
    closed: Helper<ClosedFn>,
    id: Arc<AtomicI32>,
}

#[derive(Deserialize, Serialize, Debug)]
pub struct WSParams {
    token: Option<String>, //TODO: no empty
    user: Option<String>,  //TODO: no empty
    pub(crate) symbols: Option<String>,
}

impl TwelveData {
    pub fn realtime_stream(
        &self,
    ) -> TwelveDataWebSocket<
        impl Fn(WSPrice) -> String,
        impl Fn(&WSParams, Sender<WSPrice>) -> Result<(), Status>,
        impl Fn(i32),
    > {
        println!("{}: websocket", chrono::Local::now());
        let channels = self.websocket_channels().clone();
        let ws_is_open = Arc::new(AtomicBool::new(false));
        TwelveDataWebSocket {
            ws_is_open: ws_is_open.clone(),
            id: Arc::new(AtomicI32::new(0)),
            recv: Helper {
                f: Arc::new(|p| serde_json::to_string(&p).unwrap()),
            },
            init: Helper {
                f: Arc::new(move |params: &WSParams, tx: Sender<WSPrice>| {
                    if let Some(symbols) = &params.symbols {
                        let syms: Vec<&str> = symbols.split(",").collect();
                        {
                            let mut channels = channels.lock().unwrap();
                            for sym in syms {
                                channels
                                    .entry(sym.to_owned())
                                    .or_insert(vec![])
                                    .push(tx.clone());
                            }
                        }

                        Ok(())
                    } else {
                        Err(failed!("No symbol is defined"))
                    }
                }),
            },
            closed: Helper {
                f: Arc::new(move |id| {
                    println!(">>>> Connection {id} is closed");
                }),
            },
        }
    }
}

impl<RecvFn, InitFn, ClosedFn> WebSocketServer<RecvFn, InitFn, ClosedFn>
    for TwelveDataWebSocket<RecvFn, InitFn, ClosedFn>
{
    type Packet = WSPrice;
    type Params = WSParams;

    fn is_open(&self) -> &Arc<AtomicBool> {
        &self.ws_is_open
    }
}
