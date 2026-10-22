use crossbeam_channel::*;
use serde::Serialize;
use std::{
    collections::{HashMap, VecDeque},
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Condvar, Mutex,
    },
    thread::{self, sleep, JoinHandle},
    time::{Duration, Instant},
};

use crate::{
    convert, failed,
    status::{ErrorCode, Status},
};

use super::{
    packet::{Packet, PACKET},
    Handshake,
};

#[derive(Debug)]
pub struct ConnectionInternal {
    working: Arc<AtomicBool>,
    owner_working: Arc<AtomicBool>,
    pub tx: Sender<Packet>,
    reply_queue: Arc<(Mutex<VecDeque<Packet>>, Condvar)>,
    async_reply_queue: Arc<(Mutex<VecDeque<(Packet, SocketAddr)>>, Condvar)>,

    #[allow(dead_code)]
    stderr: Option<Sender<ConnectionError>>,

    pub local_addr: SocketAddr,
    pub tcp: TcpStream,
}

pub enum ConnectionError {
    Disconnected(Option<TcpStream>),
    Other(Option<TcpStream>, String),
}

impl ConnectionInternal {
    pub fn connect(
        server_ip: &str,
        stderr: Option<Sender<ConnectionError>>,
        owner_working: Arc<AtomicBool>,
        handshake: Option<&Handshake>,
    ) -> Result<(Vec<JoinHandle<Status>>, Self), Status> {
        match crate::connect!(server_ip) {
            Ok(srv) => Self::new(srv, stderr, owner_working, handshake),
            Err(e) => Err(failed!(e, ErrorCode::Network)),
        }
    }

    pub fn disconnect(&self) -> Result<(), Status> {
        self.working.swap(false, Ordering::Relaxed);
        Ok(())
    }

    pub fn connected(&self) -> bool {
        self.working.load(Ordering::Relaxed)
    }

    pub fn new(
        tcp: TcpStream,
        stderr: Option<Sender<ConnectionError>>,
        owner_working: Arc<AtomicBool>,
        handshake: Option<&Handshake>,
    ) -> Result<(Vec<JoinHandle<Status>>, Self), Status> {
        let mut threads = vec![];
        let working = Arc::new(AtomicBool::new(true));
        let working_cloned = working.clone();
        convert!(tcp.set_read_timeout(Some(Duration::from_millis(100))))?;
        convert!(tcp.set_write_timeout(Some(Duration::from_millis(100))))?;
        let local_addr = convert!(tcp.local_addr())?;

        let (ltx, lrx) = unbounded::<Packet>(); // local channel
        let (rtx, rrx) = unbounded::<Packet>(); // remote channel

        let mut srv = convert!(tcp.try_clone())?;
        let serr = stderr.clone();
        // Packet transmitter thread
        // This thread reads packets from the remote server channel and sends them to the remote server
        let builder = thread::Builder::new().name("remote-packet-transmitter".into());
        threads.push(
            builder
                .spawn(move || {
                    while working_cloned.load(Ordering::Relaxed) {
                        match rrx.recv_timeout(Duration::from_millis(100)) {
                            Ok(msg) => {
                                let packet = msg.transmit();
                                match srv.write_all(packet.as_slice()) {
                                    Ok(_) => (), // println!("send to client {} {packet:?}", packet.len()),
                                    Err(e) => {
                                        if e.kind() != std::io::ErrorKind::WouldBlock {
                                            working_cloned.store(false, Ordering::Relaxed);
                                            if let Some(serr) = serr {
                                                let _ = serr.send(ConnectionError::Disconnected(
                                                    srv.try_clone().ok(),
                                                ));
                                            }
                                            break;
                                        }
                                    }
                                }
                            }
                            Err(RecvTimeoutError::Timeout) => (),
                            Err(_) => {
                                working_cloned.store(false, Ordering::Relaxed);
                                if let Some(serr) = serr {
                                    let _ = serr
                                        .send(ConnectionError::Disconnected(srv.try_clone().ok()));
                                }
                                break;
                            }
                        }
                    }
                    Status::successful()
                })
                .unwrap(),
        );

        let working_cloned = working.clone();
        let serr = stderr.clone();
        let mut srv = match tcp.try_clone() {
            Ok(srv) => srv,
            Err(e) => return Err(failed!(e)),
        };

        // Packet receiver thread
        // This thread receives data from the remote server and populates it in the local channel
        let builder = thread::Builder::new().name("remote-packet-receiver".into());
        threads.push(
            builder
                .spawn(move || {
                    let mut packet = Packet::new();
                    let mut rem = usize::MAX;
                    let mut receiving = 0;
                    while working_cloned.load(Ordering::Relaxed) {
                        let mut buff = PACKET;
                        match srv.read_exact(&mut buff) {
                            Ok(_) => {
                                receiving = 10000;
                                if packet.bytes.is_empty() {
                                    (packet, rem) = Packet::buildup(buff);
                                    // println!("{}", format!("initial read {} {packet:?}", packet.bytes.len()).red());
                                } else {
                                    rem = packet.append(buff, rem);
                                    // println!("{}", format!("partial read {} {packet:?}", packet.bytes.len()).yellow());
                                }
                                if rem == 0 {
                                    // println!("{}", format!("final read {} {packet:?}", packet.bytes.len()).green());
                                    if let Err(e) = ltx.send(packet) {
                                        working_cloned.store(false, Ordering::Relaxed);
                                        return failed!(e);
                                    }
                                    packet = Packet::new();
                                }
                            }
                            Err(e) => {
                                if e.kind() == std::io::ErrorKind::WouldBlock {
                                    if receiving > 0 {
                                        receiving -= 1;
                                        sleep(Duration::from_nanos(10))
                                    } else {
                                        sleep(Duration::from_millis(100))
                                    }
                                } else {
                                    working_cloned.store(false, Ordering::Relaxed);
                                    if let Some(serr) = serr {
                                        let _ = serr.send(ConnectionError::Disconnected(
                                            srv.try_clone().ok(),
                                        ));
                                    }
                                    break;
                                }
                            }
                        }
                    }
                    Status::successful()
                })
                .unwrap(),
        );

        let working_cloned = working.clone();
        let reply_queue = Arc::new((Mutex::new(VecDeque::new()), Condvar::new()));
        let queue = reply_queue.clone();
        let serr = stderr.clone();
        let srv = match tcp.try_clone() {
            Ok(srv) => srv,
            Err(e) => return Err(failed!(e)),
        };

        let (mtx, mrx) = unbounded::<(Packet, SocketAddr)>();
        // Buffering thread
        // This thread reads received data from the local channel, queue them, and
        // sends a notification that a packet is available
        let builder = thread::Builder::new().name("local-packet-receiver".into());
        threads.push(
            builder
                .spawn(move || {
                    while working_cloned.load(Ordering::Relaxed) {
                        match lrx.recv_timeout(Duration::from_millis(100)) {
                            Ok(packet) => {
                                // println!("{}", format!("received {} {packet:?}", packet.bytes.len()).blue());
                                match packet.flags() {
                                    0 => {
                                        let mut g = queue.0.lock().unwrap();
                                        g.push_back(packet);
                                        queue.1.notify_one();
                                    }
                                    1 => {
                                        let _ = mtx.send((packet, local_addr));
                                    }
                                    _ => unreachable!(),
                                }
                            }
                            Err(RecvTimeoutError::Timeout) => (),
                            Err(_) => {
                                working_cloned.store(false, Ordering::Relaxed);
                                if let Some(serr) = serr {
                                    let _ = serr
                                        .send(ConnectionError::Disconnected(srv.try_clone().ok()));
                                }
                                break;
                            }
                        }
                    }
                    Status::successful()
                })
                .unwrap(),
        );

        let async_reply_queue = Arc::new((Mutex::new(VecDeque::new()), Condvar::new()));
        let working_cloned = working.clone();
        let queue = async_reply_queue.clone();
        let serr = stderr.clone();
        let srv = match tcp.try_clone() {
            Ok(srv) => srv,
            Err(e) => return Err(failed!(e)),
        };

        // Async Buffering thread
        // This thread reads received data from the local async channel, queue them, and
        // sends a notification that a packet is available
        let builder = thread::Builder::new().name("asyn-packet-receiver".into());
        threads.push(
            builder
                .spawn(move || {
                    while working_cloned.load(Ordering::Relaxed) {
                        match mrx.recv_timeout(Duration::from_millis(100)) {
                            Ok(packet) => {
                                let mut g = queue.0.lock().unwrap();
                                g.push_back(packet);
                                queue.1.notify_one();
                            }
                            Err(RecvTimeoutError::Timeout) => (),
                            Err(_) => {
                                working_cloned.store(false, Ordering::Relaxed);
                                if let Some(serr) = serr {
                                    let _ = serr
                                        .send(ConnectionError::Disconnected(srv.try_clone().ok()));
                                }
                                break;
                            }
                        }
                    }
                    Status::successful()
                })
                .unwrap(),
        );

        let this = Self {
            working,
            owner_working,
            tx: rtx,
            reply_queue,
            async_reply_queue,
            stderr,
            local_addr,
            tcp,
        };

        // Reading the handshake and responding
        if let Some(handshake) = handshake {
            match this.recv() {
                Ok(hs) => {
                    let hs = Handshake::from(hs);
                    if let Err(e) = this.send(handshake.new_secret(hs.secret())) {
                        eprintln!("Handshake Error: {e}");
                        Err(e)
                    } else {
                        Ok((threads, this))
                    }
                }
                Err(e) => {
                    eprintln!("Handshake Error: {e}");
                    Err(e)
                }
            }
        } else {
            Ok((threads, this))
        }
    }

    pub fn send<T: Serialize>(&self, obj: T) -> Result<(), Status> {
        if self.working.load(Ordering::Relaxed) {
            convert!(self.tx.send(Packet::from(obj)), ErrorCode::Network)
        } else {
            Err(failed!("No connection", ErrorCode::Network))
        }
    }

    pub fn recv(&self) -> Result<Packet, Status> {
        let (lock, cvar) = &*self.reply_queue;
        let mut guard = lock.lock().unwrap();
        while self.working.load(Ordering::Relaxed) && self.owner_working.load(Ordering::Relaxed) {
            match cvar.wait_timeout(guard, Duration::from_millis(100)) {
                Ok((queue, _)) => {
                    guard = queue;
                    if let Some(p) = guard.pop_front() {
                        return Ok(p);
                    }
                }
                Err(e) => return Err(failed!(e)),
            }
        }
        Err(Status::successful())
    }

    pub fn request<T, R>(&self, data: T) -> Result<R, Status>
    where
        T: From<Packet> + Serialize,
        R: From<Packet> + Serialize,
    {
        self.send(data)?;
        self.recv().map(|p| R::from(p))
    }

    pub fn start_async_recv<F: Fn(Packet, SocketAddr) + Send + Sync + 'static>(
        &self,
        f: F,
    ) -> JoinHandle<Status> {
        let working_cloned = self.working.clone();
        let queue = self.async_reply_queue.clone();
        let builder = thread::Builder::new().name("asyn-packet-receiver".into());
        builder
            .spawn(move || {
                let (lock, cvar) = &*queue;
                let mut guard = lock.lock().unwrap();
                while working_cloned.load(Ordering::Relaxed) {
                    match cvar.wait_timeout(guard, Duration::from_millis(100)) {
                        Ok((queue, _)) => {
                            guard = queue;
                            if let Some((p, a)) = guard.pop_front() {
                                f(p, a)
                            }
                        }
                        Err(_) => break,
                    }
                }
                Status::successful()
            })
            .unwrap()
    }
}

#[derive(Debug)]
pub struct Connection {
    pub threads: Arc<Mutex<Vec<JoinHandle<Status>>>>,
    pub rx: Receiver<(Packet, Option<Sender<Packet>>, SocketAddr)>,
    pub tx: Sender<(Packet, Option<Sender<Packet>>, SocketAddr)>,
    pub working: Arc<AtomicBool>,
    internal: Option<Arc<ConnectionInternal>>,
    pub clients: Arc<Mutex<HashMap<SocketAddr, (TcpStream, Handshake)>>>,
}

impl Connection {
    pub fn disconnect(&self) -> Result<(), Status> {
        self.working.store(false, Ordering::Relaxed);

        if let Some(socket) = &self.internal {
            socket.disconnect()?
        }

        let mut threads = self.threads.lock().unwrap();
        while let Some(thread) = threads.pop() {
            if let Status::Failed(e) = thread.join().unwrap() {
                eprintln!("{e}");
            }
        }
        Ok(())
    }

    /// Handle the packets the remote end pushes without being asked (e.g. prices from a
    /// data server). The thread is joined on `disconnect`.
    pub fn start_async_recv<F: Fn(Packet, SocketAddr) + Send + Sync + 'static>(&self, f: F) {
        if let Some(srv) = &self.internal {
            self.threads.lock().unwrap().push(srv.start_async_recv(f));
        }
    }

    pub fn request<T, R>(&self, data: T) -> Result<R, Status>
    where
        T: From<Packet> + Serialize,
        R: From<Packet> + Serialize,
    {
        if let Some(srv) = &self.internal {
            srv.request(data)
        } else {
            Err(failed!("No connection"))
        }
    }
}

pub enum ServerConnection {
    AsServer(TcpListener),
    AsClient(String),
}

pub trait Server {
    type Error;
    type Reply;
    type Request;

    fn process(
        &mut self,
        req: Self::Request,
        addr: SocketAddr,
        req_ch: &Option<Sender<Packet>>,
    ) -> Result<Self::Reply, Self::Error>;
    fn shutdown(&mut self);
    fn shutdown_signal() -> Self::Error;
    fn shutdown_request() -> Self::Request;
    fn beat(&self) -> Result<(), Status>;
    fn conn(&self) -> &Connection;

    fn recv_timeout(
        &self,
    ) -> Result<(Packet, Option<Sender<Packet>>, SocketAddr), RecvTimeoutError> {
        // // TODO: find a way to merge recv() and recv_timeout()
        // // The problem is that some one can do send() and wait for recv(). Then, if we pop in recv(),
        // // the caller will hang. So, should not do that always.
        // if let Some(conn) = &self.conn().internal {
        //     if let Ok((p, addr)) = conn.mrx.try_recv() {
        //         return Ok((p, None, addr));
        //     }
        // }
        self.conn().rx.recv_timeout(Duration::from_millis(100))
    }

    fn start(&mut self)
    where
        Packet: From<Self::Reply>,
        Self::Error: ToString + Clone + PartialEq,
        Self::Reply: From<Self::Error>,
        Self::Request: From<Packet>,
    {
        let mut timer = Instant::now();
        loop {
            if timer.elapsed().as_secs() > 10 {
                timer = Instant::now();
                if let Err(e) = self.beat() {
                    eprintln!("Error: {e}");
                    break;
                }
            }
            match self.recv_timeout() {
                Ok((p, rtx, addr)) => match self.process(Self::Request::from(p), addr, &rtx) {
                    Ok(rep) => {
                        if let Some(rtx) = rtx {
                            let p = Packet::from(rep);
                            let _ = rtx.send(p);
                        }
                    }
                    Err(e) => {
                        if e == Self::shutdown_signal() {
                            break;
                        } else if let Some(rtx) = rtx {
                            eprintln!("Error: {}", e.to_string());
                            let _ = rtx.send(Packet::from(Self::Reply::from(e)));
                        }
                    }
                },
                Err(RecvTimeoutError::Timeout) => (),
                Err(_) => break,
            }
        }

        self.shutdown();
    }

    /// Connect to a server
    fn connect(server_addr: ServerConnection, handshake: Handshake) -> Result<Connection, Status>
    where
        Packet: From<Self::Request>,
    {
        let working = Arc::new(AtomicBool::new(true));
        let (stdout, stdout_rx) = unbounded();
        let (tx, rx) = unbounded::<(Packet, Option<Sender<Packet>>, SocketAddr)>();
        let threads = Arc::new(Mutex::new(Vec::<JoinHandle<Status>>::new()));
        let mut my_socket = None;
        let server = match server_addr {
            ServerConnection::AsClient(addr) => {
                let (mut thrds, srv) = ConnectionInternal::connect(
                    addr.as_str(),
                    Some(stdout.clone()),
                    working.clone(),
                    Some(&handshake),
                )?;
                let listener = crate::listen!(&srv.local_addr.to_string())?;
                threads.lock().unwrap().append(&mut thrds);
                let srv = Arc::new(srv);
                my_socket = Some(srv.clone());
                let tx_clone = tx.clone();
                let working_clone = working.clone();

                // Connection monitoring thread
                // This thread checks if the server is connected. If not, sends
                // a shutdown request to itself.
                let builder = thread::Builder::new().name("shutdown".into());
                let thread = builder
                    .spawn(move || {
                        while srv.connected() {
                            if !working_clone.load(Ordering::Relaxed) {
                                if let Err(e) = srv.disconnect() {
                                    return failed!(e);
                                }
                                break;
                            }
                            sleep(Duration::from_millis(100));
                        }
                        let _ = tx_clone.send((
                            Packet::from(Self::Request::from(Self::shutdown_request())),
                            None,
                            srv.local_addr,
                        ));
                        Status::successful()
                    })
                    .unwrap();
                threads.lock().unwrap().push(thread);

                Some(Arc::new(listener))
            }
            ServerConnection::AsServer(srv) => Some(Arc::new(srv)),
        };
        let clients = Arc::new(Mutex::new(
            HashMap::<SocketAddr, (TcpStream, Handshake)>::new(),
        ));
        let clients_cloned = clients.clone();
        let threads_clone = threads.clone();
        let working_clone = working.clone();
        let tx_clone = tx.clone();
        let listener = server.clone();
        if let Some(server) = listener {
            println!("Local Address: {}", server.local_addr().unwrap());
            let _ = std::io::stdout().flush();
            let _ = server.set_nonblocking(true);

            // Acceptor thread
            // This thread checks if there is a new connection to the listener
            // socket, and adds it to the clients list. It creates a new Connection
            // object for every client and spawn multiple communication threads.
            let builder = thread::Builder::new().name("acceptor".into());
            let thread = builder.spawn(move || {
                while working_clone.load(Ordering::Relaxed) {
                    if let Ok((socket, addr)) = server.accept() {
                        if let Ok(mut clients) = clients_cloned.lock() {
                            let srv = match socket.try_clone() {
                                Ok(srv) => srv,
                                Err(e) => {
                                    eprintln!("Error: {}", e.to_string());
                                    continue;
                                }
                            };
                            let tx_clone = tx_clone.clone();
                            let working_clone = working_clone.clone();
                            match ConnectionInternal::new(
                                srv,
                                Some(stdout.clone()),
                                working_clone.clone(),
                                None,
                            ) {
                                Ok((mut threads, client)) => {
                                    let secret = rand::random::<u64>();
                                    match client.request::<Handshake, Handshake>(
                                        handshake.new_secret(secret),
                                    ) {
                                        Ok(rep) => {
                                            if rep.secret() != secret {
                                                eprintln!("Handshake Error: mismatched secrets (Expected '{secret}', received '{}')", rep.secret());
                                                if let Err(e) = client.disconnect() {
                                                    return e;
                                                }
                                            }
                                            println!("{addr}: connected (type: {rep:?})");
                                            clients.insert(addr, (socket, rep));
                                            // Client receiving thread
                                            // This thread reads from the clients buffer
                                            // and propagate packets to the owners local channel.
                                            let builder = thread::Builder::new().name(format!("client-{addr}"));
                                            threads.push(builder.spawn(move || {
                                                while client.connected() {
                                                    if !working_clone.load(Ordering::Relaxed) {
                                                        if let Err(e) = client.disconnect() {
                                                            eprintln!("Error: {}", e);
                                                        }
                                                        break;
                                                    }
                                                    match client.recv() {
                                                        Ok(packet) => {
                                                            if let Err(e) = tx_clone.send((
                                                                packet,
                                                                Some(client.tx.clone()),
                                                                addr,
                                                            )) {
                                                                return failed!(e);
                                                            }
                                                        }
                                                        Err(Status::Failed(e)) => return e.into(),
                                                        Err(_) => (),
                                                    }
                                                }
                                                Status::successful()
                                            }).unwrap());
                                            threads_clone.lock().unwrap().append(&mut threads);
                                        }
                                        Err(e) => {
                                            eprintln!("Handshake Error: {e}");
                                            if let Err(e) = client.disconnect() {
                                                return e;
                                            }
                                        }
                                    }
                                }
                                Err(e) => return failed!(e),
                            }
                        }
                    } else {
                        sleep(Duration::from_millis(100))
                    }
                }
                Status::successful()
            }).unwrap();
            threads.lock().unwrap().push(thread);
        }

        let clients_cloned = clients.clone();
        let working_clone = working.clone();

        // Client connection monitoring thread
        //   * If the connection is lost, it removes it from the list clients list
        //   * If there is another error, it prints the error in stderr
        let builder = thread::Builder::new().name("client-connections".into());
        builder
            .spawn(move || {
                while working_clone.load(Ordering::Relaxed) {
                    match stdout_rx.recv_timeout(Duration::from_millis(100)) {
                        Ok(e) => match e {
                            ConnectionError::Disconnected(Some(socket)) => {
                                let mut clients = clients_cloned.lock().unwrap();
                                if let Ok(addr) = socket.peer_addr() {
                                    if clients.contains_key(&addr) {
                                        eprintln!("{}: disconnected", addr);
                                        clients.remove(&addr);
                                    }
                                }
                            }
                            ConnectionError::Disconnected(None) => {
                                eprintln!("{{unknown}}: disconnected");
                            }
                            ConnectionError::Other(Some(socket), e) => {
                                if let Ok(addr) = socket.peer_addr() {
                                    eprintln!("[Error {}]: {}", addr, e.to_string())
                                } else {
                                    eprintln!("[Error {{unknown}}]: {}", e.to_string())
                                }
                            }
                            ConnectionError::Other(None, e) => {
                                eprintln!("[Error {{unknown}}]: {}", e.to_string())
                            }
                        },
                        Err(RecvTimeoutError::Timeout) => (),
                        Err(_) => break,
                    }
                }
            })
            .unwrap();

        Ok(Connection {
            threads,
            rx,
            tx,
            working,
            internal: my_socket,
            clients,
        })
    }
}

pub fn listen_impl(
    addr: &str,
    file: &'static str,
    line: u32,
    kind: ErrorCode,
) -> Result<TcpListener, Status> {
    use net2::{unix::UnixTcpBuilderExt, TcpBuilder};
    let builder = Status::convert(TcpBuilder::new_v4(), file, line, kind)?;
    let builder = Status::convert(builder.reuse_address(true), file, line, kind)?;
    let builder = Status::convert(builder.reuse_port(true), file, line, kind)?;
    let builder = Status::convert(builder.bind(addr), file, line, kind)?;
    Status::convert(builder.listen(2), file, line, kind)
}

pub fn connect_impl(
    addr: &str,
    file: &'static str,
    line: u32,
    kind: ErrorCode,
) -> Result<TcpStream, Status> {
    use net2::{unix::UnixTcpBuilderExt, TcpBuilder};
    let builder = Status::convert(TcpBuilder::new_v4(), file, line, kind)?;
    let builder = Status::convert(builder.reuse_address(true), file, line, kind)?;
    let builder = Status::convert(builder.reuse_port(true), file, line, kind)?;
    Status::convert(builder.connect(addr), file, line, kind)
}

#[macro_export]
macro_rules! connect {
    ($e:expr,$k:path) => {
        $crate::net::connect_impl($e, std::file!(), std::line!(), $k)
    };
    ($e:expr) => {
        $crate::net::connect_impl(
            $e,
            std::file!(),
            std::line!(),
            $crate::status::ErrorCode::Generic,
        )
    };
}

#[macro_export]
macro_rules! listen {
    ($e:expr,$k:path) => {
        $crate::net::listen_impl($e, std::file!(), std::line!(), $k)
    };
    ($e:expr) => {
        $crate::net::listen_impl(
            $e,
            std::file!(),
            std::line!(),
            $crate::status::ErrorCode::Generic,
        )
    };
}
