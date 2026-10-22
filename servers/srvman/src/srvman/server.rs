use com::{colored::*, net::{Connection, Server, ServerConnection, Handshake, Packet}};
use crossbeam_channel::Sender;
use std::{
    collections::HashMap,
    io::Read,
    sync::{
        atomic::AtomicBool,
        atomic::Ordering::{self, Acquire, Release},
        Arc, Mutex, RwLock,
    },
    thread::{self},
    time::{Duration, Instant},
    {io, process::Command}, net::SocketAddr
};

use com::{
    convert, failed,
    status::Status,
};

use crate::srvman::SrvMan;

#[derive(Debug)]
/// SSHCommand structure
pub struct RemoteServer {
    user: String,
    ip: String,
    exec: String,
    cmd: Vec<String>,
    conn: Arc<Mutex<Option<Connection>>>
}

impl RemoteServer {
    pub fn new(
        exec: &str,
        user: String,
        ip: String,
        cmd: Vec<String>,
        conn: Arc<Mutex<Option<Connection>>>
    ) -> Self {
        Self {
            user,
            ip,
            cmd,
            exec: exec.to_owned(),
            conn
        }
    }
}

#[derive(Debug)]
/// HeartBeat signal monitoring system
pub struct ServiceManager {
    /// The timestamp of the last received heartbeat
    last_beats: Vec<Mutex<Instant>>,

    /// Maximum amount time to call a client dead
    timeout: Duration,

    /// Sampling frequency intervals
    interval: Duration,

    /// PID of the remote process
    pids: Vec<Mutex<u32>>,

    /// Remote servers
    pub servers: Vec<RemoteServer>,

    /// Map receiving socket address to ids to be able to restart client when disconnected
    id_map: RwLock<HashMap<String, usize>>,

    /// Working flag
    working: Arc<AtomicBool>,
}

impl ServiceManager {
    pub fn new(interval: Duration, timeout: Duration) -> Self {
        Self {
            timeout,
            interval,
            servers: vec![],
            last_beats: vec![],
            pids: vec![],
            id_map: RwLock::new(HashMap::new()),
            working: Arc::new(AtomicBool::new(true)),
        }
    }

    pub fn add(&mut self, cmd: RemoteServer) {
        // Use a large number to prevent resurrection
        self.last_beats
            .push(Mutex::new(Instant::now() + Duration::from_secs(30000000)));
        self.servers.push(cmd);
        self.pids.push(Mutex::new(0));
    }

    /// Start the server and executes the commands on each client
    pub fn start(
        self,
        addr: String,
        ready_signal: Arc<AtomicBool>,
        tx: Sender<(Packet, Option<Sender<Packet>>, SocketAddr)>
    ) -> Result<(Arc<Self>, Vec<thread::JoinHandle<()>>), Status> {
        let mut threads = vec![];
        let working = self.working.clone();
        let working_clone = working.clone();
        convert!(ctrlc::set_handler(move || {
            working_clone.store(false, Ordering::Relaxed);
            if let Err(e) = SrvMan::send_shutdown_signal(&tx) {
                eprintln!("Service Manager: Shutdown Error ({e})");
                std::process::exit(1);
            }
        }))?;

        let system = Arc::new(self);
        let listener = com::listen!(&addr)?;

        let sys = system.clone();
        // Listener thread
        let builder = thread::Builder::new().name("acceptor".to_owned());
        let addr_clone = addr.clone();
        let _ = builder.spawn(move || loop {
            match listener.accept() {
                Ok((mut socket, addr)) => {
                    println!(
                        "{}",
                        format!("HealthMonitor: new heartbeat from {addr}").blue()
                    );

                    let addr_clone = addr_clone.clone();
                    let sys = sys.clone();
                    // Receiving vital signals thread
                    let builder = thread::Builder::new().name(format!("listener-{addr}"));
                    let _ = builder.spawn(move || loop {
                        let mut buf = [0; 8];
                        match socket.read_exact(&mut buf) {
                            Ok(_) => {
                                let id = usize::from_be_bytes(buf);
                                let mut id_map = sys.id_map.write().unwrap();
                                id_map.insert(addr.to_string(), id);
                                if let Some(client) = sys.last_beats.get(id) {
                                    let mut last = client.lock().unwrap();
                                    *last = Instant::now();
                                } else {
                                    eprintln!(
                                        "{}",
                                        format!("{addr}: Id {id} is not registered").red()
                                    );
                                }
                            }
                            Err(_) => {
                                let mut id = None;
                                {
                                    // Finding the index of the disconnected client
                                    let id_map = sys.id_map.read().unwrap();
                                    if let Some(val) = id_map.get(&addr.to_string()) {
                                        id = Some(val.clone());
                                    }
                                }
                                if let Some(id) = id {
                                    // Restart the disconnected client
                                    sys.kill(id);
                                    let hist_dir = format!("{:?}", com::chrono::Utc::now());
                                    if let Err(e) = sys.execute(&addr_clone, id, &hist_dir) {
                                        eprintln!("SSH Error: {e}");
                                        // TODO: If the server is disconnected, use a backup server
                                    }
                                }
                                break;
                            }
                        }
                    });
                }
                Err(e) => {
                    eprintln!("{}", format!("HealthMonitor: {e}").red());
                    break;
                }
            }
        });

        let sys = system.clone();
        let addr_clone = addr.clone();
        // Monitoring thread
        let builder = thread::Builder::new().name("vital-monitor".to_owned());
        threads.push(
            builder
                .spawn(move || {
                    while sys.working.load(Acquire) {
                        let now = Instant::now();
                        for i in 0..sys.last_beats.len() {
                            let mut timeout = false;
                            if !sys.working.load(Acquire) {
                                break;
                            } else {
                                let mut last_beat = sys.last_beats[i].lock().unwrap();
                                if now > *last_beat + sys.timeout {
                                    eprintln!(
                                        "{}",
                                        format!(
                                            "HealthMonitor: No HB signal received \
                                            from client {i} for {} seconds (restarting)",
                                            sys.timeout.as_secs()
                                        )
                                        .red()
                                    );

                                    // Timed-out
                                    timeout = true;

                                    // Allow 30 seconds to startup to prevent double resurrecting
                                    *last_beat = now + Duration::from_secs(30);
                                }
                            }

                            if timeout {
                                sys.kill(i);
                                let hist_dir = format!("{:?}", com::chrono::Utc::now());
                                if let Err(e) = sys.execute(&addr_clone, i, &hist_dir) {
                                    eprintln!("SSH Error: {e}");
                                    // TODO: If the server is disconnected, use a backup server
                                }
                            }
                        }
                        thread::park_timeout(sys.interval);
                    }
                })
                .unwrap(),
        );

        let sys = system.clone();
        // Cluster stackup thread
        let builder = thread::Builder::new().name("stackup".to_owned());
        threads.push(
            builder
                .spawn(move || {
                    ready_signal.store(false, Ordering::Relaxed);
                    let hist_dir = format!("{:?}", com::chrono::Utc::now());
                    for idx in 0..sys.servers.len() {
                        if !working.load(Ordering::Relaxed) {
                            break;
                        }
                        println!(
                            "Executing {} on {}",
                            sys.servers[idx].cmd[0], sys.servers[idx].ip
                        );
                        if let Err(e) = sys.execute(&addr, idx, &hist_dir) {
                            eprintln!("SSH Error: {e}");
                        }
                    }
                    ready_signal.store(true, Ordering::Relaxed);
                })
                .unwrap(),
        );

        Ok((system, threads))
    }

    fn kill(&self, idx: usize) {
        let cmd = &self.servers[idx];
        let mut pid = self.pids[idx].lock().unwrap();
        if *pid != 0 {
            let output = Command::new("ssh")
                .arg(format!("{}@{}", cmd.user, cmd.ip))
                .arg(format!("kill {pid}"))
                .output()
                .expect("failed to execute process");

            if output.status.success() {
                println!(
                    "{}",
                    format!("{}@{}: Process {pid} has stopped", cmd.user, cmd.ip).green()
                );
            } else {
                println!(
                    "{}",
                    format!("{}@{}: Failed to stop process {pid}", cmd.user, cmd.ip).red()
                );
            }
            *pid = 0;

            eprint!(
                "{}",
                String::from_utf8(output.stderr)
                    .unwrap()
                    .truecolor(128, 128, 128)
            );
        }
        if let Some(conn) = &*cmd.conn.lock().unwrap() {
            if let Err(e) = conn.disconnect() {
                eprintln!("{e}")
            }
        }
    }

    fn mkdir(&self, user: &str, ip: &str, exec: &str, hist_dir: &str) -> io::Result<()> {
        // * create a folder in ~/.eztrading
        // * make a backup of old output files if exists
        // * put the pid on ~/.eztrading/datasrv.pid
        // * kill the pid of the previous run
        // * redirect the output to ~/.eztrading/datasrv.out and ~/.eztrading/datasrv.err

        let ts = format!("log-history/{hist_dir}");
        let cmds = vec![
            "mkdir -p ~/.eztrading".to_owned(),
            format!("[[ -f ~/.eztrading/{exec}.out ]] && mkdir -p ~/.eztrading/{ts} || :"),
            format!("[[ -f ~/.eztrading/{exec}.out ]] && mv ~/.eztrading/{exec}.out ~/.eztrading/{ts}/{exec}.out || :"),
            format!("[[ -f ~/.eztrading/{exec}.err ]] && mv ~/.eztrading/{exec}.err ~/.eztrading/{ts}/{exec}.err || :"),
            format!("[[ -f ~/.eztrading/{exec}.pid ]] && pkill -F ~/.eztrading/{exec}.pid || :"),
            format!("rm -f ~/.eztrading/{exec}.pid"),
        ];

        for cmd in cmds {
            let output = Command::new("ssh")
                .arg(format!("{user}@{ip}"))
                .arg(&cmd)
                .output()?;

            print!("{}", String::from_utf8(output.stdout).unwrap().yellow());
            if !output.status.success() {
                println!(
                    "{}",
                    format!("{user}@{ip}: Unable to run {exec} remotely").red()
                );
                eprint!(
                    "{}",
                    String::from_utf8(output.stderr)
                        .unwrap()
                        .truecolor(128, 128, 128)
                );
            }
        }
        Ok(())
    }

    fn execute(&self, hb_addr: &str, idx: usize, hist_dir: &str) -> Result<(), Status> {
        if !self.working.load(Ordering::Relaxed) { return Ok(()) }

        let cmd = &self.servers[idx];
        let ssh_cmd = cmd.cmd.join(" ");
        let exec = &cmd.exec;
        let user = &cmd.user;
        let ip = &cmd.ip;
        convert!(self.mkdir(user, ip, exec, hist_dir))?;

        let output = convert!(Command::new("ssh")
            .arg(format!("{user}@{ip}"))
            .arg(format!(
                "nohup {ssh_cmd} --srvman {hb_addr} --id {idx} 2> ~/.eztrading/{exec}.err > ~/.eztrading/{exec}.out & echo $!"
            ))
            .output())?;

        *self.last_beats[idx].lock().unwrap() = Instant::now() + Duration::from_secs(300);

        if output.status.success() {
            let mut pid = self.pids[idx].lock().unwrap();
            let out = String::from_utf8(output.stdout).unwrap();
            *pid = out.trim().parse().unwrap();

            thread::sleep(Duration::from_millis(100));

            let output = convert!(Command::new("ssh")
                .arg(format!("{user}@{ip}"))
                .arg(format!("kill -0 {pid}"))
                .output())?;

            if output.status.success() {
                println!(
                    "{}",
                    format!("{user}@{ip}: Process {exec} ({pid}) has started").green()
                );
                convert!(Command::new("ssh")
                    .arg(format!("{user}@{ip}"))
                    .arg(format!("echo {pid} > ~/.eztrading/{exec}.pid"))
                    .output())?;
                while Instant::now() < *self.last_beats[idx].lock().unwrap() {
                    thread::sleep(Duration::from_millis(300));
                }
                let re = com::regex::Regex::new(r"Local Address: (\d+\.\d+\.\d+.\d+:\d+)").unwrap();
                let output = convert!(Command::new("ssh")
                    .arg(format!("{user}@{ip}"))
                    .arg(format!("cat ~/.eztrading/{exec}.out"))
                    .output())?;
                let out = String::from_utf8(output.stdout).unwrap();
                if let Some(caps) = re.captures(out.as_str()) {
                    let server_addr = &caps[1];
                    println!(
                        "{} command interface address {}",
                        cmd.exec,
                        server_addr.yellow()
                    );
                    *cmd.conn.lock().unwrap() = Some(SrvMan::connect(
                        ServerConnection::AsClient(server_addr.into()),
                        Handshake::SrvMan(0),
                    )?);
                } else {
                    return Err(failed!(format!("Unable to start {} remotely", cmd.exec)));
                }
            } else {
                println!(
                    "{}",
                    format!("{user}@{ip}: Unable to run {exec} remotely").red()
                );
                let output = convert!(Command::new("ssh")
                    .arg(format!("{user}@{ip}"))
                    .arg(format!("cat ~/.eztrading/{exec}.err"))
                    .output())?;
                eprint!(
                    "{}",
                    String::from_utf8(output.stdout)
                        .unwrap()
                        .truecolor(128, 128, 128)
                );
                *pid = 0;
            }
            Ok(())
        } else {
            println!("{}", format!("{user}@{ip}: Failed").red());
            let output = convert!(Command::new("ssh")
                .arg(format!("{user}@{ip}"))
                .arg(format!("cat ~/.eztrading/{exec}.err"))
                .output())?;
            eprint!(
                "{}",
                String::from_utf8(output.stdout)
                    .unwrap()
                    .truecolor(128, 128, 128)
            );
            Ok(())
        }
    }

    pub fn shutdown(&self) {
        println!("Terminating system manager ...");
        self.working.store(false, Release);
        for i in (0..self.pids.len()).rev() {
            self.kill(i);
        }
    }
}
