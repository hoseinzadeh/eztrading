use crate::data::Ticker;
use crate::status::{ErrorCode, Status};
use crate::{convert, failed};
use crossbeam_channel::{unbounded, Receiver, RecvTimeoutError, Sender};
use serde::Deserialize;
use std::net::SocketAddr;
use std::sync::Mutex;
use std::sync::atomic::AtomicI32;
use std::{
    net::TcpStream,
    sync::{
        atomic::{AtomicBool, Ordering::Relaxed},
        Arc,
    },
    thread::{self, JoinHandle},
    time::Duration,
};
use tungstenite::{connect, stream::MaybeTlsStream, Message, WebSocket};
use url::Url;
use warp::Filter;

const MAX_ATTEMPTS: u32 = 10;

pub enum WSMessage {
    Retry,
    Reconnect,
    Disconnect,
    Heartbeat,
    Text(String),
}

pub enum WSConnMessage {
    Reconnect(String),
    Text(String),
    Ticker(String, Ticker),
}

/// A web socket client interface
///
/// This is going to be used to connect to a provider's websocket and receive
/// real-time data.
pub trait WebSocketClient {
    /// Send a heartbeat request
    fn heartbeat(&self) -> &Arc<AtomicBool>;

    /// Return true if the WS is connected
    fn connected(&self) -> &Arc<AtomicBool>;

    /// Create a new instance of Self and connect it to the websocket url
    fn create(
        symbols: Arc<Mutex<Vec<String>>>,
        stream_tx: Sender<(String, Ticker)>,
        key: &str,
    ) -> Result<(), Status>
    where
        Self: Sized;

    fn connect(url: &str) -> Result<WebSocket<MaybeTlsStream<TcpStream>>, Status> {
        let (socket, _) = convert!(connect(convert!(Url::parse(url))?), ErrorCode::Network)?;
        Ok(socket)
    }

    fn spawn(
        &self,
        mut ws: WebSocket<MaybeTlsStream<TcpStream>>,
        stream_tx: Sender<(String, Ticker)>,
    ) -> (
        JoinHandle<Result<Status, Status>>,
        Sender<WSConnMessage>,
        Receiver<WSMessage>,
    ) {
        let hb = self.heartbeat().clone();
        let (tx, rx) = crossbeam_channel::unbounded::<WSMessage>();
        let tx_clone = tx.clone();

        // Connection monitoring thread.
        // This thread periodically sends a heartbeat request to the websocket
        // to make sure of the connection being alive.
        let builder = thread::Builder::new().name("websocket".into());
        builder.spawn(move || {
            let mut attempt = 1;
            let mut reconnected = false;
            loop {
                hb.store(false, Relaxed);
                if let Err(e) = tx_clone.send(WSMessage::Heartbeat) {
                    eprintln!(
                        "{}: failed send the heartbeat request.",
                        failed!(e, ErrorCode::Network)
                    );
                    break; //TODO: Dos not reconnect after adding hb
                }
                thread::sleep(Duration::from_secs(5));
                if !hb.load(Relaxed) {
                    if attempt < MAX_ATTEMPTS {
                        attempt += 1;
                        reconnected = false;
                        if let Err(e) = tx_clone.send(WSMessage::Reconnect) {
                            eprintln!(
                                "{}: failed to send the reconnection request.",
                                failed!(e, ErrorCode::Network)
                            );
                            break;
                        }
                        thread::sleep(Duration::from_secs(5));
                        continue;
                    }
                    if !reconnected {
                        eprintln!("please check your network connection");
                        if let Err(e) = tx_clone.send(WSMessage::Disconnect) {
                            eprintln!(
                                "{}: failed to send the disconnection request.",
                                failed!(e, ErrorCode::Network)
                            );
                            break;
                        }
                        break;
                    }
                } else {
                    reconnected = true;
                    attempt = 1;
                }
            }
        }).unwrap();

        let (rtx, rrx) = crossbeam_channel::unbounded::<WSConnMessage>();
        let is_connected = self.connected().clone();
        match ws.get_ref() {
            MaybeTlsStream::Plain(s) => s.set_read_timeout(Some(Duration::from_secs(1))).unwrap(),
            MaybeTlsStream::NativeTls(s) => s
                .get_ref()
                .set_read_timeout(Some(Duration::from_secs(1)))
                .unwrap(),
            _ => (),
        }
        let builder = thread::Builder::new().name("websocket-cmd".into());
        let thrd = builder.spawn(move || {
            while is_connected.load(Relaxed) {
                match rrx.recv_timeout(Duration::from_millis(1)) {
                    Ok(WSConnMessage::Text(s)) => {
                        if let Err(e) = ws.write_message(Message::Text(s)) {
                            return Err(failed!(e, ErrorCode::Network));
                        }
                    }
                    Ok(WSConnMessage::Reconnect(url)) => {
                        match Self::connect(url.as_str()) {
                            Ok(w) => ws = w,
                            Err(e) => return Err(e),
                        };
                        match ws.get_ref() {
                            MaybeTlsStream::Plain(s) => {
                                s.set_read_timeout(Some(Duration::from_secs(1))).unwrap()
                            }
                            MaybeTlsStream::NativeTls(s) => s
                                .get_ref()
                                .set_read_timeout(Some(Duration::from_secs(1)))
                                .unwrap(),
                            _ => (),
                        }
                        if let Err(e) = tx.send(WSMessage::Retry) {
                            return Err(failed!(e, ErrorCode::Network));
                        }
                    }
                    Ok(WSConnMessage::Ticker(sym, t)) => {
                        if let Err(e) = stream_tx.send((sym, t)) {
                            return Err(failed!(e, ErrorCode::Network));
                        }
                    }
                    Err(RecvTimeoutError::Disconnected) => {
                        return Err(failed!("Websocket error: connection closed", ErrorCode::Network))
                    }
                    Err(_) => (),
                }
                match ws.read_message() {
                    Ok(m) => {
                        if let Err(e) = tx.send(WSMessage::Text(m.to_string())) {
                            return Err(failed!(e, ErrorCode::Network));
                        }
                    }
                    Err(tungstenite::Error::ConnectionClosed) => {
                        return Err(failed!("Websocket error: connection closed", ErrorCode::Network))
                    }
                    Err(_) => (),
                }
            }
            Ok(Status::successful())
        }).unwrap();
        (thrd, rtx, rx)
    }
}

/// A web socket server interface
///
/// This is going to be used by clients to connect to the server
/// (particularly [`DataSrv`]), and receive real-time data.
pub trait WebSocketServer<RecvFn, InitFn, ClosedFn> {
    type Packet: Send + Clone + 'static;
    type Params: Send + Sync + for<'de> Deserialize<'de> + 'static;

    fn is_open(&self) -> &Arc<AtomicBool>;

    fn run(
        &mut self,
        _cert: &str,
        _key: &str,
        addr: impl Into<SocketAddr>,
        recv: Arc<RecvFn>,
        init: Arc<InitFn>,
        closed: Arc<ClosedFn>,
        id: Arc<AtomicI32>,
    ) -> Result<(), Status>
    where
        Self: Sized,
        InitFn:
            Fn(&Self::Params, Sender<Self::Packet>) -> Result<(), Status> + Sync + Send + 'static,
        RecvFn: Fn(Self::Packet) -> String + Sync + Send + 'static,
        ClosedFn: Fn(i32) + Sync + Send + 'static,
    {
        use futures::{SinkExt, StreamExt};
        use warp::ws::WebSocket;

        async fn handle_client<Data, Recv, Closed>(
            id: Arc<AtomicI32>,
            ws: WebSocket,
            rx: Receiver<Data>,
            is_working: Arc<AtomicBool>,
            recv: Arc<Recv>,
            closed: Arc<Closed>,
        ) where
            Recv: Fn(Data) -> String,
            Closed: Fn(i32),
        {
            let (mut sink, _stream) = ws.split();
            let my_id = id.fetch_add(1, Relaxed);
            while is_working.load(Relaxed) {
                match rx.recv_timeout(Duration::from_secs(5)) {
                    Ok(val) => {
                        let msg = warp::ws::Message::text(recv(val));
                        match sink.send(msg).await {
                            Ok(_) => (),
                            Err(e) => {
                                eprintln!("websocket send error: {e}");
                                break;
                            }
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => (),
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
            if let Err(e) = sink.close().await {
                eprintln!("{e}")
            }
            closed(my_id);
            eprintln!("websocket: ConnectionClosed");
        }

        let is_working = self.is_open().clone();
        is_working.store(true, Relaxed);
        let _addr: SocketAddr = addr.into();

        let _routes = warp::path("price")
            .and(warp::query::<Self::Params>())
            .and(warp::ws())
            .map(move |params: Self::Params, ws: warp::ws::Ws| {
                let is_working = is_working.clone();
                let init = init.clone();
                let recv = recv.clone();
                let closed = closed.clone();
                let id = id.clone();
                let (tx, rx) = unbounded();
                ws.on_upgrade(move |ws| match init(&params, tx) {
                    Ok(_) => handle_client(id, ws, rx, is_working, recv, closed),
                    Err(e) => {
                        panic!("{e}");
                    }
                })
            });

        // let (num_tokio_worker_threads, max_tokio_blocking_threads) = (3000, 512); // 512 is tokio's current default
        // let rt = convert!(tokio::runtime::Builder::new_multi_thread()
        //     .enable_all()
        //     .thread_stack_size(1024 * 1024)
        //     .worker_threads(num_tokio_worker_threads)
        //     .max_blocking_threads(max_tokio_blocking_threads)
        //     .build())?;

        // let cert = cert.to_owned();
        // let key = key.to_owned();
        // thread::spawn(move || {
        //     rt.block_on({
        //         warp::serve(warp::get().and(routes))
        //             // .tls()
        //             // .cert_path(cert)
        //             // .key_path(key)
        //             .run(addr)
        //     });
        //     std::mem::forget(rt);
        // });
        Ok(())
    }
}
