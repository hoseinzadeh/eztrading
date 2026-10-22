use super::market::Market;
use super::TimeSeries;
use super::{search::Filter, series::Series, Ticker};
use crate::net::{Packet, Symbol, WebSocketClient};
use crate::status::{ErrorCode, Status};
use crate::{convert, failed};
use crossbeam_channel::{RecvTimeoutError, Sender};
use std::net::SocketAddr;
use std::sync::atomic::Ordering::Relaxed;
use std::{
    collections::HashMap,
    fmt::Display,
    path::Path,
    sync::{atomic::AtomicBool, Arc, Mutex},
    thread::{self, JoinHandle},
    time::Duration,
};

// TODO: Make it multi tiered
pub const HISTORY_SIZE: usize = 20000;

/// Data Provider Trait
///
/// The struct implementing this trait is the code object that performs the following tasks:
///   1. Connects to a data provider websocket like twelvedata.com
///   2. Creates a [`Series`] object by fetching the history of the given symbols
///   3. Manage the cache
pub trait Provider {
    type WS: WebSocketClient;
    type WSData: Send + Sync + From<Ticker> + 'static;

    /// Return the workspace path
    fn workspace(&self) -> &str;

    /// Return the workspace path
    fn set_workspace<S: ToString>(&mut self, path: S);

    /// Returns a mutable reference to the list of symbols
    fn symbols(&self) -> &Arc<Mutex<Vec<String>>>;

    /// Returns a shared reference to the streams object
    fn streams(&self) -> &Arc<HashMap<String, Mutex<Series>>>;

    /// Returns a mutable reference to the stream object
    fn set_streams(&mut self, streams: Arc<HashMap<String, Mutex<Series>>>);

    /// Returns an immutable reference to the threads vector
    fn threads(&self) -> &Arc<Mutex<Vec<JoinHandle<Status>>>>;

    /// Returns the time series of the given symbols
    fn retrieve_history(&mut self, filter: Filter) -> Result<HashMap<String, TimeSeries>, Status>;

    /// Returns the time series of the given symbols
    fn earliest_timestamps(
        &mut self,
        filter: Filter,
    ) -> Result<HashMap<String, Result<i64, String>>, Status>;

    /// Returns true if the provider is running
    fn is_working(&self) -> &Arc<AtomicBool>;

    /// Creates a default instance of the provider
    fn create<S1: ToString, S2: ToString>(workspace: S1, apikey: S2) -> Self;

    /// Stop working
    fn stop(&self);

    /// Self Initialization
    fn init(&mut self, api_key: &str) -> Result<(), Status>;

    /// Return an immutable reference to the markets map
    fn markets(&self) -> &Arc<HashMap<String, Market<Self>>>;

    /// Start the WebSocket service to allow client receive real-time data
    fn start_websocket_service(
        &self,
        cert: &str,
        key: &str,
        addr: impl Into<SocketAddr>,
    ) -> Result<(), Status>;

    /// Return the map of all transmitting channels belonging to the end users (WebSocket)
    fn websocket_channels(&self) -> &Arc<Mutex<HashMap<Symbol, Vec<Sender<Self::WSData>>>>>;

    /// Return the map of all transmitting channels belonging to the listeners (JobSrv)
    fn listeners(&self) -> &Arc<Mutex<HashMap<Symbol, Vec<Sender<Packet>>>>>;

    /// Process the ticker and prepare a packet to send to the listener
    fn send_to_listener(sym: &String, t: Ticker) -> Packet;

    /// Creates and launch a new provider
    fn launch<'a, S: AsRef<Path> + Display>(
        workspace: S,
        symbols: Vec<S>,
        api_key: String,
        cert: &str,
        key: &str,
        addr: impl Into<SocketAddr>,
    ) -> Result<Self, Status>
    where
        Self: Sized + Default + Send + 'static,
    {
        convert!(std::fs::create_dir_all(&workspace), ErrorCode::FS)?;
        use std::fs::OpenOptions;

        let mut this = Self::default(); // = convert!(bincode::deserialize(slice), ErrorCode::IO)?;
        *this.symbols().lock().unwrap() = symbols.iter().map(|s| s.to_string()).collect();
        this.set_workspace(workspace);
        this.is_working().store(true, Relaxed);
        this.init(api_key.as_str())?;

        let dataset_filename = format!("{}/dataset.dat", this.workspace());
        let dataset = convert!(
            OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .open(dataset_filename),
            ErrorCode::FS
        )?;
        let mut len = dataset.metadata().unwrap().len() as usize;
        let mut initialize = false;
        let sector_size = Series::size_of(HISTORY_SIZE);
        if len < 8 {
            initialize = true;
            len = this.symbols().lock().unwrap().len() * sector_size;
            dataset.set_len(len as u64).unwrap();
        }
        let mut memmap =
            unsafe { convert!(memmap2::MmapOptions::new().map_mut(&dataset), ErrorCode::FS)? };
        let mut base = memmap.as_mut_ptr();
        std::mem::forget(dataset);
        std::mem::forget(memmap);

        let mut streams = HashMap::new();
        let binding = this.symbols().clone();
        let syms = binding.lock().unwrap();
        for sym in &*syms {
            let stream;
            (stream, base) = Series::new(sym.clone(), base, initialize, HISTORY_SIZE);
            streams.insert(sym.clone(), Mutex::new(stream));
        }

        let streams = Arc::new(streams);
        this.set_streams(streams.clone());

        let (tx, rx) = crossbeam_channel::unbounded::<(String, Ticker)>();
        let working = this.is_working().clone();
        let channels = this.websocket_channels().clone();
        let listeners = this.listeners().clone();
        let builder = thread::Builder::new().name("realtime-stream".into());
        let hdl = Some(
            builder
                .spawn(move || {
                    let mut code = Status::successful();
                    while working.load(Relaxed) {
                        match rx.recv_timeout(Duration::from_secs(1)) {
                            Ok((sym, t)) => {
                                println!(
                                    "{sym} [{}]: {} ({})",
                                    chrono::NaiveDateTime::from_timestamp_opt(t.datetime as i64, 0)
                                        .unwrap(),
                                    t.close,
                                    t.volume
                                );

                                if let Some(list) = listeners.lock().unwrap().get_mut(&sym) {
                                    let mut remove_later = vec![];
                                    let pack = Self::send_to_listener(&sym, t);
                                    for i in 0..list.len() {
                                        if let Err(e) = list[i].send(pack.clone()) {
                                            remove_later.push(i);
                                            eprintln!("{e}")
                                        }
                                    }
                                    if !remove_later.is_empty() {
                                        println!("remove_later = {remove_later:?}");
                                        for i in remove_later.iter().rev() {
                                            list.remove(*i);
                                        }
                                    }
                                }

                                // TODO: Send t to every JobSrv and websocket subscriber
                                if let Some(list) = channels.lock().unwrap().get_mut(&sym) {
                                    let mut remove_later = vec![];
                                    for i in 0..list.len() {
                                        // println!("Sending to {i}");
                                        if let Err(e) = list[i].send(Self::WSData::from(t)) {
                                            remove_later.push(i);
                                            eprintln!("{e}")
                                        }
                                    }
                                    if !remove_later.is_empty() {
                                        // println!("remove_later = {remove_later:?}");
                                        for i in remove_later.iter().rev() {
                                            list.remove(*i);
                                        }
                                    }
                                };
                            }
                            Err(RecvTimeoutError::Disconnected) => {
                                code = failed!("Connection lost", ErrorCode::Network);
                                working.store(false, Relaxed);
                            }
                            Err(_) => (),
                        }
                    }
                    code
                })
                .unwrap(),
        );

        let working = this.is_working().clone();
        let builder = thread::Builder::new().name("realtime-websocket".into());
        let symbols = this.symbols().clone();
        builder
            .spawn(move || {
                while working.load(Relaxed) {
                    match Self::WS::create(symbols.clone(), tx.clone(), &api_key) {
                        Ok(_) => (),
                        Err(e) => {
                            eprintln!("{e}");
                            eprintln!("Attempting to reconnect ...");
                            thread::sleep(Duration::from_secs(5));
                        }
                    }
                }
            })
            .unwrap();

        this.start_websocket_service(cert, key, addr)?;
        this.threads().lock().unwrap().push(hdl.unwrap());
        Ok(this)
    }
}

mod test {
    #[test]
    fn provider() {}
}
