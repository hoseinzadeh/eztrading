use colored::Colorize;
use com::data::search::Filter;
use com::data::Ticker;
use com::failed;
use com::status::Status;
use com::{
    data::provider::Provider,
    net::{man::HeartBeat, *},
};
use crossbeam_channel::{RecvTimeoutError, Sender};
use std::collections::HashMap;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::RwLock;
use std::{
    net::{SocketAddr, TcpListener},
    sync::{atomic::AtomicBool, Arc},
    thread::{self, JoinHandle},
    time::Duration,
};

pub struct DataSrv<P> {
    conn: Connection,
    hb: Option<HeartBeat>,
    is_working: Arc<AtomicBool>,
    provider_thread: Option<JoinHandle<Status>>,
    provider: Arc<RwLock<P>>,
}

impl<P: Provider + Default> DataSrv<P> {
    pub fn new<'a>(
        socket: TcpListener,
        srvman: Option<(String, usize)>,
        workspace: &str,
        api_key: &str,
        cert: &str,
        key: &str,
        addr: impl Into<SocketAddr>,
    ) -> Result<Self, Status>
    where
        P: Send + Sync + 'static,
    {
        let is_working = Arc::new(AtomicBool::new(true));
        let is_working_clone = is_working.clone();
        let provider = Arc::new(RwLock::new(P::default()));
        let provider_clone = provider.clone();
        let api_key = api_key.to_owned();
        let workspace = workspace.to_owned();
        let cert = cert.to_owned();
        let key = key.to_owned();
        let addr: SocketAddr = addr.into();
        let builder = thread::Builder::new().name("provider".into());
        let provider_thread = Some(builder.spawn(move || {
            while is_working_clone.load(Relaxed) {
                match P::launch(
                    format!("{workspace}/twelvedata").as_str(),
                    vec!["AAPL", "BTC/USD"],
                    api_key.clone(),
                    &cert,
                    &key,
                    addr,
                ) {
                    Ok(p) => {
                        let is_working = p.is_working().clone();
                        *provider_clone.write().unwrap() = p;
                        {
                            // Spin here until stop working
                            while is_working.load(Relaxed) {
                                {
                                    let p = provider_clone.read().unwrap();
                                    let threads = p.threads();
                                    for h in &*threads.lock().unwrap() {
                                        if h.is_finished() {
                                            is_working.store(false, Relaxed);
                                            break;
                                        }
                                    }
                                }
                                thread::sleep(Duration::from_secs(1));
                            }
                        }
                        loop {
                            if let Ok(p) = provider_clone.read() {
                                let threads = p.threads();
                                while let Some(h) = threads.lock().unwrap().pop() {
                                    if let Ok(e) = h.join() {
                                        match e {
                                            Status::Successful => (),
                                            Status::Failed(e) => return e.into(),
                                            Status::Multi(_) => return e,
                                        }
                                    }
                                }
                                break;
                            }
                        }
                    }
                    Err(e) => eprintln!("{e}"),
                }
                eprintln!("Provider connection lost");
                if is_working_clone.load(Relaxed) {
                    std::thread::sleep(Duration::from_secs(5));
                }
            }
            Status::Successful
        }).unwrap());
        Ok(Self {
            conn: Self::connect(
                ServerConnection::AsServer(socket),
                Handshake::DataSrv(0),
            )?,
            hb: if let Some(sys) = srvman {
                Some(HeartBeat::new(sys.0.as_str(), sys.1)?)
            } else {
                None
            },
            is_working,
            provider,
            provider_thread,
        })
    }

    /// Returns the time series of the given symbol cached in the system
    pub fn time_series(&self, filter: &Filter) -> Result<HashMap<Symbol, Vec<Ticker>>, Status> {
        let streams = {
            let p = self.provider.read().map_err(|e| failed!(e))?;
            p.streams().clone()
        };
        let mut res = HashMap::new();
        for sym in &filter.symbols {
            if let Some(stream) = streams.get(sym) {
                let mut stream = stream.lock().unwrap();
                let mut p = self.provider.write().map_err(|e| failed!(e))?;
                res.insert(sym.clone(), stream.fetch(filter, &mut *p)?);
            }
        }
        Ok(res)
    }

    fn register_listener(
        &mut self,
        addr: SocketAddr,
        symbols: &Vec<String>,
        req_ch: &Option<Sender<Packet>>,
    ) -> Result<DataSrvRep, DataSrvError> {
        if let Some(req_ch) = req_ch {
            if !symbols.is_empty() {
                let provider = self.provider.read().unwrap();
                let mut listeners = provider.listeners().lock().unwrap();
                println!(
                    "{}",
                    format!("{addr} has started listening to {symbols:?}").cyan()
                );
                // TODO: Make sure it does not send data before successful signal
                for sym in symbols {
                    listeners
                        .entry(sym.clone())
                        .or_default()
                        .push(req_ch.clone())
                }
                Ok(DataSrvRep::Successful)
            } else {
                Err(DataSrvError::Err(failed!("No symbol is specified")))
            }
        } else {
            Err(DataSrvError::Err(failed!("Unable to bind reply channel")))
        }
    }
}

impl<P: Provider + Default> Server for DataSrv<P> {
    type Error = DataSrvError;
    type Reply = DataSrvRep;
    type Request = DataSrvReq;

    fn process(
        &mut self,
        req: DataSrvReq,
        addr: SocketAddr,
        req_ch: &Option<Sender<Packet>>,
    ) -> Result<Self::Reply, Self::Error> {
        match &req {
            DataSrvReq::Log(msg) => {
                println!("{}: {}", addr, msg);
                Ok(DataSrvRep::Successful)
            }
            DataSrvReq::List(lst) => match lst {
                DataSrvListReq::Users => Ok(DataSrvRep::Todo(req)),
                DataSrvListReq::Clients => Ok(DataSrvRep::Clients(
                    self.conn
                        .clients
                        .lock()
                        .unwrap()
                        .values()
                        .map(|(addr, hs)| match hs {
                            Handshake::JobSrv(_) => format!("jobsrv@{}", addr.peer_addr().unwrap()),
                            Handshake::CLI(_) => format!("cli@{}", addr.peer_addr().unwrap()),
                            _ => unreachable!(),
                        })
                        .collect(),
                )),
            },
            DataSrvReq::Listen(sym) => self.register_listener(addr, sym, req_ch),
            DataSrvReq::HeartBeat => Ok(DataSrvRep::HeartBeat),
            DataSrvReq::Shutdown => Err(DataSrvError::Shutdown),
            DataSrvReq::TimeSeries(filter) => match self.time_series(filter) {
                Ok(series) => Ok(DataSrvRep::TimeSeries(series)),
                Err(e) => Err(DataSrvError::Err(e)),
            },
            DataSrvReq::EarliestTimestamp(filter) => match self
                .provider
                .write()
                .unwrap()
                .earliest_timestamps(filter.clone())
            {
                Ok(rep) => Ok(DataSrvRep::EarliestTimestamp(rep)),
                Err(e) => Err(DataSrvError::Err(e)),
            },
        }
    }

    fn shutdown(&mut self) {
        println!("Shutting down...");
        self.is_working.store(false, Relaxed);
        {
            let p = self.provider.read().unwrap();
            p.is_working().store(false, Relaxed);
        }
        if let Some(h) = self.provider_thread.take() {
            if let Ok(Status::Failed(e)) = h.join() {
                eprintln!("{e}")
            }
        }
        self.conn.disconnect().unwrap();
    }

    fn recv_timeout(
        &self,
    ) -> Result<(Packet, Option<Sender<Packet>>, SocketAddr), RecvTimeoutError> {
        self.conn.rx.recv_timeout(Duration::from_secs(1))
    }

    fn shutdown_signal() -> Self::Error {
        DataSrvError::Shutdown
    }

    fn shutdown_request() -> Self::Request {
        DataSrvReq::Shutdown
    }

    fn beat(&self) -> Result<(), Status> {
        if let Some(hb) = &self.hb {
            hb.beat()
        } else {
            Ok(())
        }
    }

    fn conn(&self) -> &Connection {
        &self.conn
    }
}
