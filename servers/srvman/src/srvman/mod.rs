use com::chrono::NaiveDate;
use com::mysql::prelude::Queryable;
use com::mysql::{params, PooledConn};
use com::net::{
    CodeSrvRep, CodeSrvReq, Connection, Handshake, JobId, JobSrvListReq, JobSrvRep, JobSrvReq,
    Packet, PluginId, Server, ServerConnection, SrvManError, SrvManListReq, SrvManRep, SrvManReq,
    UserId,
};
use com::status::Status;
use com::{convert, failed};
use crossbeam_channel::Sender;
use serde::Deserialize;
use server::{RemoteServer, ServiceManager};
use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{JoinHandle, Thread};
use std::time::{Duration, Instant};
use std::{process, thread};

mod server;

#[derive(Deserialize, Debug)]
pub struct SrvMan {
    pub name: Option<String>,
    pub system: System,
    pub database: Database,
    pub servers: Servers,

    #[serde(skip)]
    srvman: Option<Arc<ServiceManager>>,

    #[serde(skip)]
    threads: Arc<Mutex<Vec<JoinHandle<()>>>>,

    #[serde(skip)]
    conn: Option<Connection>,

    #[serde(skip)]
    pub ready: Arc<AtomicBool>,

    #[serde(skip)]
    tasks: TaskManager,

    #[serde(skip)]
    db_conn_str: String,

    #[serde(skip)]
    db: Option<PooledConn>,
}

#[derive(Deserialize, Debug)]
pub struct System {
    pub host: String,
    pub port: u16,
    pub hb_port: u16,
    pub http_port: u16,
    pub interval: u64,
    pub timeout: u64,
}

#[derive(Deserialize, Debug)]
pub struct Database {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub pswd: String,
}

#[derive(Deserialize, Debug)]
pub struct Servers {
    pub datasrv: DataServer,
    pub jobsrv: HashMap<String, JobServer>,
    pub codesrv: HashMap<String, CodeServer>,
}

#[derive(Deserialize, Debug)]
pub struct DataServer {
    pub host: String,
    pub exec: String,
    pub port: u16,
    pub user: String,
    pub apikey: Option<String>,
    pub workspace: Option<String>,

    #[serde(skip)]
    conn: Arc<Mutex<Option<Connection>>>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct CodeServer {
    pub host: String,
    pub exec: String,
    pub port: u16,
    pub user: String,
    pub workspace: String,
    pub template: String,
    pub build_script: String,

    #[serde(skip)]
    conn: Arc<Mutex<Option<Connection>>>,
}

#[derive(Deserialize, Debug)]
pub struct JobServer {
    pub host: String,
    pub exec: String,
    pub user: String,
    pub cap: Option<u32>,
    pub workspace: Option<String>,
    pub schlib: Option<String>,

    #[serde(skip)]
    conn: Arc<Mutex<Option<Connection>>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
enum ServiceType {
    Compilation,
    Deployment,
}

#[derive(Clone, Debug, Default)]
pub struct TaskManager {
    last_job_id: u64,
    servers: HashMap<ServiceType, HashMap<String, ServiceProvider>>,
}

#[derive(Clone, Debug)]
pub struct ServiceProvider {
    conn: Arc<Mutex<Option<Connection>>>,
    last_access: Arc<Mutex<Instant>>,
    job_queue: Arc<(Mutex<VecDeque<(JobId, SrvManReq, Sender<Packet>)>>, Condvar)>,
}

impl ServiceProvider {
    fn new(conn: Arc<Mutex<Option<Connection>>>) -> Self {
        Self {
            conn,
            last_access: Arc::new(Mutex::new(Instant::now())),
            job_queue: Default::default(),
        }
    }
}

impl SrvMan {
    pub fn new() -> Result<Self, Status> {
        use com::db::Database;
        let mut this: Self = convert!(toml::from_str(
            convert!(std::fs::read_to_string("system.toml"))?.as_str()
        ))?;
        this.db_conn_str = format!(
            "mysql://{}:{}@{}:{}/eztrading",
            this.database.user, this.database.pswd, this.database.host, this.database.port
        );
        this.db = Some(Self::new_pool(&this.db_conn_str)?);
        let listener = com::listen!(&format!("{}:{}", this.system.host, this.system.port))?;
        this.ready.store(false, Ordering::Relaxed);
        this.conn = Some(Self::connect(
            ServerConnection::AsServer(listener),
            Handshake::SrvMan(0),
        )?);

        if let Some(name) = &this.name {
            println!("Starting up {}", name);
        }

        let mut srvman = ServiceManager::new(
            Duration::from_secs(this.system.interval),
            Duration::from_secs(this.system.timeout),
        );

        let dbsrv = &this.database;
        let db = format!(
            "--db {}:{} --db-user {} --db-password {}",
            dbsrv.host, dbsrv.port, dbsrv.user, dbsrv.pswd
        );

        let datasrv = &this.servers.datasrv;
        let datasrv_addr = format!("{}:{}", datasrv.host, datasrv.port);
        let apikey = datasrv
            .apikey
            .as_ref()
            .map(|t| format!("--apikey {t}"))
            .unwrap_or("".to_owned());
        let workspace = datasrv
            .workspace
            .as_ref()
            .map(|t| format!("--workspace {t}"))
            .unwrap_or("".to_owned());
        srvman.add(RemoteServer::new(
            "datasrv",
            datasrv.user.clone(),
            datasrv.host.clone(),
            vec![
                datasrv.exec.clone(),
                "--ip".to_owned(),
                datasrv_addr.clone(),
                db.clone(),
                apikey,
                workspace,
            ],
            datasrv.conn.clone(),
        ));

        for (id, codesrv) in &this.servers.codesrv {
            let codesrv_addr = format!("{}:{}", codesrv.host, codesrv.port);
            srvman.add(RemoteServer::new(
                &format!("codesrv-{id}"),
                codesrv.user.clone(),
                codesrv.host.clone(),
                vec![
                    codesrv.exec.clone(),
                    "--ip".into(),
                    codesrv_addr,
                    db.clone(),
                    "--workspace".into(),
                    codesrv.workspace.clone(),
                    "--template".into(),
                    codesrv.template.clone(),
                    "--build-script".into(),
                    codesrv.build_script.clone(),
                ],
                codesrv.conn.clone(),
            ));
        }

        for (id, jobsrv) in &this.servers.jobsrv {
            let mut args = vec![
                jobsrv.exec.clone(),
                "--datasrv".to_owned(),
                datasrv_addr.clone(),
                db.clone(),
            ];
            if let Some(cap) = &jobsrv.cap {
                args.push(format!("-c {cap}"));
            }
            if let Some(ws) = &jobsrv.workspace {
                args.push(format!("-w {ws}"));
            }
            if let Some(sch) = &jobsrv.schlib {
                args.push(format!("--sch-lib {sch}"));
            }
            srvman.add(RemoteServer::new(
                &format!("jobsrv-{id}"),
                jobsrv.user.clone(),
                jobsrv.host.clone(),
                args,
                jobsrv.conn.clone(),
            ));
        }

        let (sys, threads) = srvman.start(
            format!("{}:{}", this.system.host, this.system.hb_port),
            this.ready.clone(),
            this.tx_channel(),
        )?;

        this.tasks
            .servers
            .insert(ServiceType::Compilation, Default::default());
        for (name, codesrv) in &this.servers.codesrv {
            this.tasks
                .servers
                .get_mut(&ServiceType::Compilation)
                .unwrap()
                .insert(name.clone(), ServiceProvider::new(codesrv.conn.clone()));
        }

        this.tasks
            .servers
            .insert(ServiceType::Deployment, Default::default());
        for (name, jobsrv) in &this.servers.jobsrv {
            this.tasks
                .servers
                .get_mut(&ServiceType::Deployment)
                .unwrap()
                .insert(name.clone(), ServiceProvider::new(jobsrv.conn.clone()));
        }

        this.srvman = Some(sys.clone());
        this.threads = Arc::new(Mutex::new(threads));
        this.start_request_management_thread();

        Ok(this)
    }

    fn start_request_management_thread(&mut self) {
        for (_, list) in &self.tasks.servers {
            for (id, srv) in list {
                let mut conn = srv.conn.clone();
                let job_queue = srv.job_queue.clone();
                let builder = thread::Builder::new().name(format!("{id}-request-management"));
                let _ = builder.spawn(move || {
                    let (queue, cvar) = &*job_queue;
                    loop {
                        let first = {
                            let mut queue = queue.lock().unwrap();
                            if let Some(val) = queue.pop_front() {
                                Some(val)
                            } else {
                                drop(cvar.wait(queue));
                                None
                            }
                        };
                        if let Some((job_id, req, ch)) = first {
                            let reply = match &req {
                                SrvManReq::Upload(upload) => Self::codesrv_request(
                                    &mut conn,
                                    job_id,
                                    CodeSrvReq::Upload(upload.clone()),
                                ),
                                SrvManReq::Build(plugin_id, publish, price) => {
                                    Self::codesrv_request(
                                        &mut conn,
                                        job_id,
                                        CodeSrvReq::Build(
                                            *plugin_id,
                                            *publish,
                                            price.clone(),
                                            job_id,
                                        ),
                                    )
                                }
                                SrvManReq::LoadPlugin(plugin_id, force_download) => {
                                    Self::jobsrv_request(
                                        &mut conn,
                                        job_id,
                                        JobSrvReq::LoadPlugin(*plugin_id, *force_download),
                                    )
                                }
                                SrvManReq::Launch(user_id, machine) => Self::jobsrv_request(
                                    &mut conn,
                                    job_id,
                                    JobSrvReq::Launch(*user_id, machine.clone()),
                                ),
                                _ => todo!(),
                            };

                            // Sending the reply back to the requester
                            match reply {
                                Ok(rep) => {
                                    let p = Packet::from(rep).async_process();
                                    let _ = ch.send(p);
                                }
                                Err(e) => {
                                    eprintln!("Error: {}", e.to_string());
                                    let _ = ch.send(Packet::from(SrvManRep::from(e)));
                                }
                            }
                        }
                    }
                });
            }
        }
    }

    pub fn tx_channel(&self) -> Sender<(Packet, Option<Sender<Packet>>, SocketAddr)> {
        self.conn.as_ref().unwrap().tx.clone()
    }

    pub fn send_shutdown_signal(
        tx: &Sender<(Packet, Option<Sender<Packet>>, SocketAddr)>,
    ) -> Result<(), Status> {
        convert!(tx.send((
            Packet::from(SrvManReq::Shutdown),
            None,
            SocketAddr::from(([127, 0, 0, 1], 1000))
        )))
    }

    fn codesrv_request(
        conn: &mut Arc<Mutex<Option<Connection>>>,
        job_id: u64,
        req: CodeSrvReq,
    ) -> Result<SrvManRep, SrvManError> {
        if let Some(conn) = &*conn.lock().unwrap() {
            match conn.request::<CodeSrvReq, CodeSrvRep>(req) {
                Err(e) => Err(SrvManError::Err(e)),
                Ok(rep) => Ok(SrvManRep::CodeSrv(job_id, rep)),
            }
        } else {
            Ok(SrvManRep::Error(SrvManError::ConnectionLost))
        }
    }

    fn jobsrv_request(
        conn: &mut Arc<Mutex<Option<Connection>>>,
        job_id: u64,
        req: JobSrvReq,
    ) -> Result<SrvManRep, SrvManError> {
        if let Some(conn) = &*conn.lock().unwrap() {
            match conn.request::<JobSrvReq, JobSrvRep>(req) {
                Err(e) => Err(SrvManError::Err(e)),
                Ok(rep) => Ok(SrvManRep::JobSrv(job_id, rep)),
            }
        } else {
            Ok(SrvManRep::Error(SrvManError::ConnectionLost))
        }
    }

    fn round_robin(services: &mut HashMap<String, ServiceProvider>) -> Option<String> {
        let mut id = None;
        let mut min = Instant::now();
        for (my_id, codesrv) in services.iter() {
            let last_access = codesrv.last_access.lock().unwrap();
            if *last_access < min {
                min = *last_access;
                id = Some(my_id.clone());
            }
        }
        id
    }

    /// Only for JobSrvs
    fn idle_first(services: &mut HashMap<String, ServiceProvider>) -> Option<String> {
        let mut id = None;
        let mut min = usize::MAX;
        for (my_id, jobsrv) in services.iter() {
            if let Ok(conn) = jobsrv.conn.lock() {
                if let Some(conn) = &*conn {
                    if let Ok(JobSrvRep::RunningJobs(n)) = conn.request(JobSrvReq::RunningJobs) {
                        if n < min {
                            min = n;
                            id = Some(my_id.clone());
                        }
                    }
                }
            }
        }
        id
    }

    /// Submit a request to a service provider of the given type who was accessed earlier
    fn submit<F>(
        &mut self,
        ty: ServiceType,
        req: SrvManReq,
        reply_ch: Option<Sender<Packet>>,
        f: F,
    ) -> Result<SrvManRep, SrvManError>
    where
        F: Fn(&mut HashMap<String, ServiceProvider>) -> Option<String>,
    {
        self.tasks.last_job_id += 1;
        let services = self.tasks.servers.get_mut(&ty).unwrap();
        let id = f(services).clone();
        if let Some(id) = id {
            if let Some(srv) = services.get_mut(&id) {
                self.tasks.last_job_id += 1;
                let job_id = self.tasks.last_job_id;
                Self::submit_one(job_id, srv, req, reply_ch)
            } else {
                Err(SrvManError::Err(failed!(format!(
                    "No {ty:?} service provider with id {id} is running"
                ))))
            }
        } else {
            Ok(SrvManRep::Error(SrvManError::Err(failed!(format!(
                "No {ty:?} service provider is running"
            )))))
        }
    }

    /// Submit a request to a service provider of the given type
    fn submit_one(
        job_id: u64,
        srv: &mut ServiceProvider,
        req: SrvManReq,
        reply_ch: Option<Sender<Packet>>,
    ) -> Result<SrvManRep, SrvManError> {
        if let Some(ch) = reply_ch {
            srv.job_queue.0.lock().unwrap().push_back((job_id, req, ch));
            *srv.last_access.lock().unwrap() = Instant::now();
            srv.job_queue.1.notify_one();
            Ok(SrvManRep::Submitted(vec![job_id]))
        } else {
            Err(SrvManError::Err(failed!("No reply channel")))
        }
    }

    /// Submit a request to all service providers of the given type
    fn submit_all(
        &mut self,
        ty: ServiceType,
        req: SrvManReq,
        reply_ch: Option<Sender<Packet>>,
    ) -> Result<SrvManRep, SrvManError> {
        let mut jobs = vec![];
        let mut services = self.tasks.servers.get_mut(&ty).unwrap().clone();
        for (_, srv) in services.iter_mut() {
            self.tasks.last_job_id += 1;
            let job_id = self.tasks.last_job_id;
            if let Ok(SrvManRep::Submitted(mut v)) =
                Self::submit_one(job_id, srv, req.clone(), reply_ch.clone())
            {
                jobs.append(&mut v);
            }
        }
        Ok(SrvManRep::Submitted(jobs))
    }

    fn subscribe(
        &mut self,
        user_id: UserId,
        plugin_id: PluginId,
        renew: bool,
        expire: NaiveDate,
    ) -> Result<SrvManRep, SrvManError> {
        if let Some(db_conn) = &mut self.db {
            db_conn
                .exec_drop(
                    r"INSERT INTO subscriptions (user_id, plugin_id, renew, expire)
                VALUES (:user_id, :plugin_id, :renew, :expire)",
                    params! {
                        "user_id" => &user_id,
                        "plugin_id" => &plugin_id,
                        "renew" => &renew,
                        "expire" => &expire,
                    },
                )
                .map_err(|e| SrvManError::Err(failed!(e)))?;
            Ok(SrvManRep::Successful)
        } else {
            Err(SrvManError::Err(failed!("No DB Connection")))
        }
    }

    fn unsubscribe(
        &mut self,
        user_id: UserId,
        plugin_id: PluginId,
    ) -> Result<SrvManRep, SrvManError> {
        if let Some(db_conn) = &mut self.db {
            db_conn
                .exec_drop(
                    r"DELETE FROM subscriptions WHERE user_id=:user_id AND plugin_id=:plugin_id",
                    params! {
                        "user_id" => &user_id,
                        "plugin_id" => &plugin_id,
                    },
                )
                .map_err(|e| SrvManError::Err(failed!(e)))?;
            Ok(SrvManRep::Successful)
        } else {
            Err(SrvManError::Err(failed!("No DB Connection")))
        }
    }
}

impl com::db::Database for SrvMan {
    fn db_conn(&mut self) -> &mut Option<PooledConn> {
        &mut self.db
    }

    fn db_conn_str(&self) -> &str {
        &self.db_conn_str
    }
}

impl Server for SrvMan {
    type Error = SrvManError;
    type Reply = SrvManRep;
    type Request = SrvManReq;

    fn process(
        &mut self,
        req: Self::Request,
        addr: std::net::SocketAddr,
        req_ch: &Option<Sender<Packet>>,
    ) -> Result<Self::Reply, Self::Error> {
        use ServiceType::*;
        match &req {
            SrvManReq::Log(msg) => {
                println!("{}: {}", addr, msg);
                Ok(SrvManRep::Successful)
            }
            SrvManReq::Upload(_) => {
                self.submit(Compilation, req, req_ch.clone(), Self::round_robin)
            }
            SrvManReq::Build(_, _, _) => {
                self.submit(Compilation, req, req_ch.clone(), Self::round_robin)
            }
            SrvManReq::LoadPlugin(_, _) => self.submit_all(Deployment, req, req_ch.clone()),
            SrvManReq::Launch(_, _) => {
                self.submit(Deployment, req, req_ch.clone(), Self::idle_first)
            }
            SrvManReq::List(SrvManListReq::Machines) => {
                if let Some((_, jobsrv)) = self.servers.jobsrv.iter_mut().last() {
                    Self::jobsrv_request(
                        &mut jobsrv.conn,
                        0,
                        JobSrvReq::List(JobSrvListReq::Machines),
                    )
                } else {
                    Err(SrvManError::Err(failed!("No JobSrv is running")))
                }
            }
            SrvManReq::List(_) => todo!(),
            SrvManReq::Shutdown => Err(SrvManError::Shutdown),
            SrvManReq::Subscribe(user_id, plugin_id, renew, expire) => {
                self.subscribe(*user_id, *plugin_id, *renew, *expire)
            }
            SrvManReq::Unsubscribe(user_id, plugin_id) => self.unsubscribe(*user_id, *plugin_id),
        }
    }

    fn shutdown(&mut self) {
        if let Some(c) = &self.conn {
            if let Err(e) = c.disconnect() {
                eprintln!("{e}")
            }
        }

        self.srvman.as_ref().unwrap().shutdown();
        let t_handles: Vec<Thread> = self
            .threads
            .lock()
            .unwrap()
            .iter()
            .map(|t| t.thread().clone())
            .collect();

        for t in &t_handles {
            let _ = t.unpark();
        }

        while let Some(t) = self.threads.lock().unwrap().pop() {
            let _ = t.join();
        }

        println!("Service Manager: Clean Shutdown");
        process::exit(0);
    }

    fn shutdown_signal() -> Self::Error {
        SrvManError::Shutdown
    }

    fn shutdown_request() -> Self::Request {
        SrvManReq::Shutdown
    }

    fn beat(&self) -> Result<(), com::status::Status> {
        Ok(())
    }

    fn conn(&self) -> &com::net::Connection {
        self.conn.as_ref().unwrap()
    }
}
