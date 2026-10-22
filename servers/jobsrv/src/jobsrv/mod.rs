use chrono::{NaiveDate, Utc};
use com::{
    api::{PluginAPI, SchApi},
    convert,
    data::{Machine, Param, Ticker},
    db::Database,
    dylib_file_extension, failed,
    net::{man::HeartBeat, *},
    status::{ErrorCode, Status},
};
use crossbeam_channel::*;
use dlopen::wrapper::Container;
use mysql::{prelude::Queryable, PooledConn, params, TxOpts};
use sch::Scheduler;
use std::{collections::{HashMap, HashSet}, fs::OpenOptions, io::Write, net::SocketAddr, path::Path};

pub mod sch;

pub struct Plugin {
    api: PluginAPI,
    name: String,
    active_users: HashSet<UserId>,
}

pub struct Run {
    user_id: u32,
    plugin_id: u32,
    machine_name: String,
    machine_json: String,
    sch_idx: usize,
    handle: *mut std::ffi::c_void,
}

impl Run {
    fn new(user_id: u32, plugin_id: u32, machine_name: String, machine_json: String, sch_idx: usize, handle: *mut std::ffi::c_void) -> Run {
        Self { user_id, plugin_id, machine_name, machine_json, sch_idx, handle }
    }
}

pub struct JobSrv {
    pub api: Container<SchApi>,
    schedulers: Vec<Scheduler>,
    runs: HashMap<RunId, Run>,
    plugins: HashMap<PluginId, Plugin>,
    datasrv_conn: Connection,
    hb: Option<HeartBeat>,
    sch_idx: usize,
    workspace: String,
    db_conn_str: String,
    db: Option<PooledConn>,
    /// The id srvman gave this server (bots are stored with it so each server restores only its own)
    server_id: Option<usize>,
}

unsafe impl Send for JobSrv {}

impl Database for JobSrv {
    fn db_conn(&mut self) -> &mut Option<PooledConn> {
        &mut self.db
    }

    fn db_conn_str(&self) -> &str {
        &self.db_conn_str
    }
}

impl JobSrv {
    pub fn new(
        mut scheduler_lib_path: String,
        datasrv_ip: &str,
        db_addr: String,
        db_user: String,
        db_pass: String,
        workspace: String,
        srvman: Option<(String, usize)>,
    ) -> Result<Self, Status> {
        let server_id = srvman.as_ref().map(|s| s.1);
        let datasrv_conn = Self::connect(ServerConnection::AsClient(datasrv_ip.into()), Handshake::JobSrv(0))?;
        // The data server pushes prices without being asked; feed them to our own request loop.
        {
            let tx = datasrv_conn.tx.clone();
            datasrv_conn.start_async_recv(move |p, addr| {
                let _ = tx.send((p, None, addr));
            });
        }
        let plugins_dir = format!("{}/plugins", workspace);
        if !Path::new(&plugins_dir).exists() {
            convert!(std::fs::create_dir_all(plugins_dir))?;
        }
        scheduler_lib_path += "\0";
        let db_conn_str = format!("mysql://{db_user}:{db_pass}@{db_addr}/eztrading");
        Ok(Self {
            api: SchApi::new(scheduler_lib_path)?,
            schedulers: vec![],
            runs: HashMap::new(),
            plugins: HashMap::new(),
            sch_idx: 0,
            workspace,
            hb: if let Some(sys) = srvman {
                Some(HeartBeat::new(sys.0.as_str(), sys.1)?)
            } else {
                None
            },
            datasrv_conn,
            db_conn_str: format!("mysql://{db_user}:{db_pass}@{db_addr}/eztrading"),
            db: Some(Self::new_pool(&db_conn_str)?),
            server_id,
        })
    }

    fn plugin_subscribers(&mut self, plugin_id: u32) -> Result<Vec<(UserId, NaiveDate)>, Status> {
        if let Some(db_conn) = self.db_conn() {
            convert!(db_conn
                .query(format!("SELECT user_id,expire from subscriptions WHERE plugin_id={plugin_id}")))
        } else {
            Err(failed!("No DB Connection"))

        }
    }

    fn download_dylib(
        &mut self,
        filename: &str,
        plugin_id: u32,
        force: bool,
    ) -> Result<PluginName, Status> {
        if let Some(db_conn) = self.db_conn() {
            if force || !Path::new(&filename).exists() {
                let data: Option<(PluginName, Option<Vec<u8>>)> = convert!(db_conn
                    .query_first(format!("SELECT name,lib from plugins WHERE id={plugin_id}")))?;
                if let Some((name, Some(data))) = data {
                    // save to file
                    let mut file = convert!(OpenOptions::new()
                        .create(true)
                        .write(true)
                        .truncate(true)
                        .open(&filename))?;
                    // Write a slice of bytes to the file
                    convert!(file.write_all(data.as_slice()))?;
                    Ok(name)
                } else if let Some((name, None)) = data {
                    Err(failed!(
                        format!("Plugin '{name}' (id={plugin_id}) has not been compiled, yet."),
                        ErrorCode::Database
                    ))
                } else {
                    Err(failed!(
                        format!("No plugin with id {plugin_id}"),
                        ErrorCode::Database
                    ))
                }
            } else {
                let name: Option<PluginName> = convert!(db_conn
                    .query_first(format!("SELECT name from plugins WHERE id={plugin_id}")))?;
                if let Some(name) = name {
                    Ok(name)
                } else {
                    Err(failed!(
                        format!("No plugin with id {plugin_id}"),
                        ErrorCode::Database
                    ))
                }
            }
        } else {
            Err(failed!("No DB Connection"))
        }
    }

    pub fn load_plugin(
        &mut self,
        plugin_id: u32,
        force_download: bool,
    ) -> Result<&mut Plugin, Status> {
        if let Some(plugin) = self.plugins.get_mut(&plugin_id) {
            if !force_download {
                return Ok(self.plugins.get_mut(&plugin_id).unwrap());
            }
            if !plugin.active_users.is_empty() {
                return Err(failed!(format!("Plugin {plugin_id} is being used by {} users.", plugin.active_users.len())));
            }
        }
        // Every load writes a new file: overwriting a library that is mapped into this process
        // crashes it (SIGBUS), and a rebuilt plugin has to replace the one that is loaded.
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
        let dir = format!("{}/plugins", self.workspace);
        let mut lib_path = format!("{dir}/plugin_{plugin_id}_{stamp}{}", dylib_file_extension());
        println!("Downloading plugin from {lib_path} ...");
        let name = self.download_dylib(&lib_path, plugin_id, true)?;
        let previous = self.plugins.remove(&plugin_id);
        lib_path += "\0";
        let mut api = PluginAPI::new();
        unsafe {
            use com::libc;
            println!(">> loading plugin ...");
            let hdl = libc::dlopen(lib_path.as_bytes().as_ptr() as *mut i8, libc::RTLD_LAZY);
            if self.api.load(hdl, &mut api.handle) {
                api.cnt = self.api.get_contents(&api);
            } else {
                eprintln!("Plugin load failed");
                if let Some(p) = previous {
                    self.plugins.insert(plugin_id, p);
                }
                return Err(failed!(format!("Plugin {plugin_id} could not be loaded")));
            }
        }
        // the previous copy stays mapped (its machines are stopped), but its file is no longer needed
        if let Ok(rd) = std::fs::read_dir(&dir) {
            let keep = lib_path.trim_end_matches('\0').rsplit('/').next().unwrap_or("").to_owned();
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().into_owned();
                if n.starts_with(&format!("plugin_{plugin_id}")) && n != keep {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
        self.plugins.insert(plugin_id, Plugin { api, name, active_users: HashSet::new() });
        self.restore_bots(plugin_id);
        Ok(self.plugins.get_mut(&plugin_id).unwrap())
    }

    /// After a restart: load the plugins of the bots stored for this server and start them again.
    pub fn restore(&mut self) {
        let server_id = self.server_id.map(|i| i as i64);
        let ids: Vec<u32> = match self.db_conn() {
            Some(conn) => conn
                .exec("SELECT DISTINCT plugin_id FROM bots WHERE server_id <=> :s", params! { "s" => server_id })
                .unwrap_or_default(),
            None => return,
        };
        for id in ids {
            if let Err(e) = self.load_plugin(id, false) {
                eprintln!("Could not restore the bots of plugin {id}: {e}");
            }
        }
    }

    /// Start the bots stored for this plugin and server that are not running yet (after a restart).
    fn restore_bots(&mut self, plugin_id: u32) {
        if self.schedulers.is_empty() {
            return;
        }
        let server_id = self.server_id.map(|i| i as i64);
        let rows: Vec<(u32, u32, String)> = match self.db_conn() {
            Some(conn) => conn
                .exec(
                    "SELECT id, user_id, machine FROM bots WHERE plugin_id = :p AND server_id <=> :s",
                    params! { "p" => plugin_id, "s" => server_id },
                )
                .unwrap_or_default(),
            None => return,
        };
        for (run_id, user_id, json) in rows {
            if self.runs.contains_key(&run_id) {
                continue;
            }
            match serde_json::from_str::<Machine>(&json) {
                Ok(machine) => match self.relaunch(&machine, user_id, run_id) {
                    Ok(_) => {
                        println!("Restored bot {run_id} ({}) for user {user_id}", machine.name);
                        self.listen_to(&machine);
                    }
                    Err(e) => eprintln!("Could not restore bot {run_id}: {e}"),
                },
                Err(e) => eprintln!("Could not read bot {run_id}: {e}"),
            }
        }
    }

    /// Ask the data server for the symbols a machine works with (its `Vec<Symbol>` parameters).
    fn listen_to(&mut self, machine: &Machine) {
        let mut symbols: Vec<String> = vec![];
        for p in &machine.params {
            if p.r#type.contains("Symbol") {
                let bytes: Vec<u8> = (0..p.value.data.len() / 2)
                    .filter_map(|i| u8::from_str_radix(&p.value.data[2 * i..2 * i + 2], 16).ok())
                    .take(p.value.size)
                    .collect();
                if let Ok(text) = String::from_utf8(bytes) {
                    symbols.extend(text.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned));
                }
            }
        }
        symbols.sort();
        symbols.dedup();
        if !symbols.is_empty() {
            if let Err(e) = self.datasrv_conn.request::<DataSrvReq, DataSrvRep>(DataSrvReq::Listen(symbols.clone())) {
                eprintln!("Could not subscribe to {symbols:?}: {e}");
            } else {
                println!("Listening to {symbols:?}");
            }
        }
    }

    /// Hand a new price to every loaded plugin and wake the jobs that wait for it.
    fn push_ticker(&mut self, symbol: &str, t: &Ticker) {
        let Ok(sym) = std::ffi::CString::new(symbol) else { return };
        if std::env::var("EZT_TRACE_TICKS").is_ok() {
            println!("tick {symbol} {:.4} -> {} plugin(s)", t.close, self.plugins.len());
        }
        let handles: Vec<*mut std::ffi::c_void> = self.schedulers.iter().map(|s| s.handle()).collect();
        for plugin in self.plugins.values() {
            unsafe {
                self.api.update_ticker(
                    plugin.api.handle() as *const _,
                    sym.as_ptr(),
                    t.datetime,
                    t.open,
                    t.high,
                    t.low,
                    t.close,
                    t.volume as i32,
                    handles.as_ptr(),
                    handles.len(),
                );
            }
        }
    }

    pub fn get_plugin_id(&mut self, machine: &str) -> Result<PluginId, Status> {
        if self.schedulers.is_empty() {
            Err(failed!("No scheduler is running"))
        } else if let Some((_, plugin_id)) = self.machines().get(machine) {
            Ok(*plugin_id)
        } else {
            Err(failed!(format!(
                "No such machine name as '{}'",
                machine
            )))
        }
    }

    /// Launch a machine without saving it to the database (loading) and return the scheduler index
    pub fn relaunch(&mut self, machine: &Machine, user_id: u32, run_id: u32) -> Result<usize, Status> {
        if self.schedulers.is_empty() {
            Err(failed!("No scheduler is running"))
        } else if let Some((_, plugin_id)) = self.machines().get(&machine.name) {
            // `machine` keeps the parameters the user chose (not the plugin's defaults)
            self.sch_idx = (self.sch_idx + 1) % self.schedulers.len();
            let subscribers = self.plugin_subscribers(*plugin_id)?.clone();
            let subscribers: HashMap<UserId, NaiveDate> = subscribers.into_iter().collect();
            let plugin = self.plugins.get_mut(&plugin_id).unwrap();
            if let Some(exp) = subscribers.get(&user_id) {
                if exp >= &Utc::now().naive_utc().date() {
                    let sch = &mut self.schedulers[self.sch_idx];
                    let machine_name = format!("{}\0", machine.name);
                    let machine_json = format!("{}\0", serde_json::to_string(&machine).unwrap());
                    plugin.active_users.insert(user_id);
                    let handle = unsafe {
                        self.api.launch_machine(
                            plugin.api.handle,
                            user_id,
                            sch.handle(),
                            machine_name.as_ptr() as *mut i8,
                            machine_json.as_ptr() as *mut i8,
                        )
                    };
                    sch.add_machine(handle)?;
                    self.runs.insert(run_id, Run::new(user_id, *plugin_id, machine_name, machine_json, self.sch_idx, handle));
                    Ok(self.sch_idx)
                } else {
                    Err(failed!(format!("User {user_id}'s subscription to plugin {plugin_id} has been expired")))
                }
            } else {
                Err(failed!(format!("User {user_id} does not subscribe to plugin {plugin_id}")))
            }
        } else {
            Err(failed!(format!(
                "No such machine name as '{}'",
                machine.name
            )))
        }
    }

    /// Launch a machine and save it to the database
    pub fn launch(&mut self, machine: &Machine, user_id: u32) -> Result<usize, Status> {
        let plugin_id = self.get_plugin_id(&machine.name)?;
        let server_id = self.server_id.map(|i| i as i64);
        let this = self as *mut Self;
        if let Some(db_conn) = self.db_conn() {
            if let Ok(mut txn) = db_conn.start_transaction(TxOpts::default()) {
                convert!(
                    txn.exec_drop(
                        r"INSERT INTO bots (user_id, plugin_id, machine, server_id)
                        VALUES (:user_id, :plugin_id, :machine, :server_id)",
                        params! {
                            "user_id" => &user_id,
                            "plugin_id" => &plugin_id,
                            "machine" => serde_json::to_string(machine).unwrap(),
                            "server_id" => server_id,
                        },
                    ),
                    ErrorCode::Database
                )?;
                let run_id: Option<u32> = convert!(txn.query_first("SELECT LAST_INSERT_ID()"))?;
                match unsafe {&mut *this}.relaunch(machine, user_id, run_id.ok_or(failed!("DB Error"))?) {
                    Ok(idx) => {
                        convert!(txn.commit())?;
                        unsafe {&mut *this}.listen_to(machine);
                        Ok(idx)
                    }
                    Err(e) => {
                        convert!(txn.rollback())?;
                        Err(e)
                    }
                }
            } else {
                Err(failed!("Could not start a transaction"))
            }
        } else {
            Err(failed!("No DB Connection"))
        }
    }

    pub fn stop(&mut self, run_id: u32) -> Result<(), Status> {
        let Some(run) = self.runs.remove(&run_id) else {
            return Err(failed!(format!("No bot {run_id} is running on this server")));
        };
        println!("Stopping bot {run_id} for user {} (machine {} from plugin {})", run.user_id, run.machine_name.trim_end_matches('\0'), run.plugin_id);
        unsafe { self.api.stop_machine(run.handle) };
        if let Some(sch) = self.schedulers.get_mut(run.sch_idx) {
            sch.forget_machine(run.handle);
        }
        if let Some(db_conn) = self.db_conn() {
            convert!(db_conn.exec_drop(r"DELETE FROM bots WHERE id=:run_id", params! { "run_id" => run_id }), ErrorCode::Database)?;
        }
        if !self.runs.values().any(|r| r.user_id == run.user_id && r.plugin_id == run.plugin_id) {
            if let Some(p) = self.plugins.get_mut(&run.plugin_id) {
                p.active_users.remove(&run.user_id);
            }
        }
        Ok(())
    }

    pub fn setup(&mut self, count: usize) {
        for _ in 0..count {
            self.schedulers
                .push(Scheduler::new(unsafe { self.api.new_scheduler() }));
        }
    }

    pub fn plugins(&self) -> HashMap<PluginId, (PluginName, Vec<Machine>)> {
        let mut map = HashMap::new();
        for (id, plugin) in &self.plugins {
            map.insert(*id, (plugin.name.clone(), plugin.api.machines().clone()));
        }
        map
    }

    pub fn all_plugins(&mut self) -> JobSrvRep {
        if let Some(db_conn) = self.db_conn() {
            match db_conn.query_map(
                "SELECT plugins.id, user_id, users.name, users.lastname, users.email, plugins.name, machines, version,
                price, plugins.date, published from plugins inner JOIN users on users.id = plugins.user_id",
                |(id, owner_id, user_name, user_lastname, owner_email, name, machines, version, price, date, published)| {
                    let machines: Option<String> = machines;
                    let user_name: String = user_name;
                    let user_lastname: String = user_lastname;
                    let mut machines: Option<HashMap<String, Vec<Machine>>> = machines.map(|s| serde_json::from_str(&s).unwrap());
                    (
                        id,
                        PluginDBEntry {
                            name,
                            owner_id,
                            owner_name: format!("{user_name} {user_lastname}"),
                            owner_email,
                            version,
                            price,
                            date,
                            published,
                            machines: if let Some(m) = &mut machines { Some(m.remove("machines").unwrap()) } else { None }
                        }
                    )
                }
            ) {
                Ok(res) => JobSrvRep::AllPlugins(res.into_iter().collect()),
                Err(e) => JobSrvRep::Error(JobSrvError::Err(failed!(e, ErrorCode::Database)))                
            }
        } else {
            JobSrvRep::Error(JobSrvError::Err(failed!("No DB Connection", ErrorCode::Database)))
        }
    }

    pub fn machines(&self) -> HashMap<MachineName, (Machine, PluginId)> {
        let mut map = HashMap::new();
        for (plugin_id, plugin) in &self.plugins {
            for m in plugin.api.machines() {
                map.insert(m.name.clone(), (m.clone(), *plugin_id));
            }
        }
        map
    }

    pub fn plugin_machines(&self, plugin_id: u32) -> Option<Vec<Machine>> {
        for (id, p) in &self.plugins {
            if *id == plugin_id {
                return Some(p.api.machines().clone());
            }
        }
        None
    }

    pub fn params(&self, machine_name: &str) -> Option<Vec<Param>> {
        for (_, p) in &self.plugins {
            for m in p.api.machines() {
                if m.name == machine_name {
                    return Some(m.params.clone());
                }
            }
        }
        None
    }

    pub fn job_count(&self) -> usize {
        let mut ret_val = 0;
        for sch in &self.schedulers {
            ret_val += unsafe { self.api.job_count(sch.handle()) };
        }
        ret_val
    }
}

impl Server for JobSrv {
    type Error = JobSrvError;
    type Reply = JobSrvRep;
    type Request = JobSrvReq;

    fn process(
        &mut self,
        req: JobSrvReq,
        addr: SocketAddr,
        _req_ch: &Option<Sender<Packet>>,
    ) -> Result<Self::Reply, Self::Error> {
        match &req {
            JobSrvReq::Log(msg) => {
                println!("{}: {}", addr, msg);
                Ok(JobSrvRep::Successful)
            }
            JobSrvReq::Listen(symbols) => {
                if let Err(e) = self
                    .datasrv_conn
                    .request::<DataSrvReq, DataSrvRep>(DataSrvReq::Listen(symbols.clone()))
                {
                    Err(JobSrvError::Err(failed!(e)))
                } else {
                    Ok(JobSrvRep::Successful)
                }
            }
            JobSrvReq::Update(sym, t) => {
                self.push_ticker(sym, t);
                Ok(JobSrvRep::Successful)
            }
            JobSrvReq::Emit(_) => Ok(JobSrvRep::Todo(req)),
            JobSrvReq::CopyData(_, _) => Ok(JobSrvRep::Todo(req)),
            JobSrvReq::LoadPlugin(id, download) => match self.load_plugin(*id, *download) {
                Ok(p) => Ok(JobSrvRep::Plugin(p.name.clone(), p.api.machines().clone())),
                Err(e) => Err(JobSrvError::Err(e)),
            },
            JobSrvReq::Launch(uid, sm) => match self.launch(sm, *uid) {
                Ok(_) => Ok(JobSrvRep::Successful),
                Err(e) => Err(JobSrvError::Err(e)),
            },
            JobSrvReq::Stop(run_id) => match self.stop(*run_id) {
                Ok(_) => Ok(JobSrvRep::Successful),
                Err(e) => Err(JobSrvError::Err(e)),
            },
            JobSrvReq::List(JobSrvListReq::Machines) => Ok(
                JobSrvRep::Machines(
                    self.machines()
                    .into_iter()
                    .map(|(n,(m,_))| (n,m)).collect()
                )
            ),
            JobSrvReq::List(JobSrvListReq::Plugins) => Ok(JobSrvRep::Plugins(self.plugins())),
            JobSrvReq::List(JobSrvListReq::AllPlugins) => Ok(self.all_plugins()),
            JobSrvReq::List(JobSrvListReq::PluginMachines(plugin_id)) => {
                if let Some(res) = self.plugin_machines(*plugin_id) {
                    Ok(JobSrvRep::MachinesList(res))
                } else {
                    Err(JobSrvError::Err(failed!(format!(
                        "No plugin found with id '{plugin_id}'"
                    ))))
                }
            }
            JobSrvReq::List(JobSrvListReq::MachineParams(m)) => {
                if let Some(res) = self.params(m) {
                    Ok(JobSrvRep::Params(res))
                } else {
                    Err(JobSrvError::Err(failed!(format!(
                        "No machine found with name '{m}'"
                    ))))
                }
            }
            JobSrvReq::RunningJobs => Ok(JobSrvRep::RunningJobs(self.job_count())),
            JobSrvReq::Shutdown => Err(JobSrvError::Shutdown),
        }
    }

    fn shutdown(&mut self) {
        println!("Shutting down...");
        let _ = self.datasrv_conn.disconnect();

        for sch in &self.schedulers {
            unsafe { self.api.stop(sch.handle()) }
        }

        for sch in &self.schedulers {
            unsafe { self.api.join(sch.handle()) }
        }
    }

    fn shutdown_signal() -> Self::Error {
        JobSrvError::Shutdown
    }

    fn shutdown_request() -> Self::Request {
        JobSrvReq::Shutdown
    }

    fn beat(&self) -> Result<(), Status> {
        if let Some(hb) = &self.hb {
            hb.beat()
        } else {
            Ok(())
        }
    }

    fn conn(&self) -> &Connection {
        &self.datasrv_conn
    }
}
