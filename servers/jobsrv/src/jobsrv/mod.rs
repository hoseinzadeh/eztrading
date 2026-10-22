use chrono::{NaiveDate, Utc};
use com::{
    api::{PluginAPI, SchApi},
    convert,
    data::{Machine, Param},
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
    sch_idx: usize
}

impl Run {
    fn new(user_id: u32, plugin_id: u32, machine_name: String, machine_json: String, sch_idx: usize) -> Run {
        Self { user_id, plugin_id, machine_name, machine_json, sch_idx }
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
            datasrv_conn: Self::connect(
                ServerConnection::AsClient(datasrv_ip.into()),
                Handshake::JobSrv(0),
            )?,
            db_conn_str: format!("mysql://{db_user}:{db_pass}@{db_addr}/eztrading"),
            db: Some(Self::new_pool(&db_conn_str)?)
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
        if force_download {
            if let Some(plugin) = self.plugins.get_mut(&plugin_id) {
                if plugin.active_users.is_empty() {
                    self.plugins.remove(&plugin_id);
                } else {
                    return Err(failed!(format!("Plugin {plugin_id} is being used by {} users.", plugin.active_users.len())));
                }
            }
        }
        let mut lib_path = format!(
            "{}/plugins/plugin_{}{}",
            self.workspace,
            plugin_id,
            dylib_file_extension()
        );
        println!("Downloading plugin from {lib_path} ...");
        let name = self.download_dylib(&lib_path, plugin_id, force_download)?;
        lib_path += "\0";
        let plugin = self.plugins.entry(plugin_id).or_insert_with(|| {
            let mut api = PluginAPI::new();
            unsafe {
                use com::libc;
                println!(">> loading plugin ...");

                let hdl = libc::dlopen(lib_path.as_bytes().as_ptr() as *mut i8, libc::RTLD_LAZY);
                if self
                    .api
                    .load(hdl, &mut api.handle)
                {
                    api.cnt = self.api.get_contents(&api);
                } else {
                    eprintln!("Plugin load failed");
                }
            }
            Plugin { api, name, active_users: HashSet::new() }
        });
        Ok(plugin)
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
        } else if let Some((machine, plugin_id)) = self.machines().get(&machine.name) {
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
                    unsafe {
                        sch.add_machine(self.api.launch_machine(
                            plugin.api.handle,
                            user_id,
                            sch.handle(),
                            machine_name.as_ptr() as *mut i8,
                            machine_json.as_ptr() as *mut i8,
                        ))?;
                    }
                    self.runs.insert(run_id, Run::new(user_id, *plugin_id, machine_name, machine_json, self.sch_idx));
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
        let this = self as *mut Self;
        if let Some(db_conn) = self.db_conn() {
            if let Ok(mut txn) = db_conn.start_transaction(TxOpts::default()) {
                convert!(
                    txn.exec_drop(
                        r"INSERT INTO bots (user_id, plugin_id, machine)
                        VALUES (:user_id, :plugin_id, :machine)",
                        params! {
                            "user_id" => &user_id,
                            "plugin_id" => &plugin_id,
                            "machine" => serde_json::to_string(machine).unwrap(),
                        },
                    ),
                    ErrorCode::Database
                )?;
                let run_id: Option<u32> = convert!(txn.query_first("SELECT LAST_INSERT_ID()"))?;
                match unsafe {&mut *this}.relaunch(machine, user_id, run_id.ok_or(failed!("DB Error"))?) {
                    Ok(idx) => Ok(idx),
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
        let this = self as *mut Self;
        if let Some(db_conn) = self.db_conn() {
            if let Ok(mut txn) = db_conn.start_transaction(TxOpts::default()) {
                convert!(
                    txn.exec_drop(
                        r"DELETE FROM bots WHERE id=:run_id",
                        params! { "run_id" => run_id },
                    ),
                    ErrorCode::Database
                )?;
                let this = unsafe { &mut *this };
                if let Some(run) = this.runs.remove(&run_id) {
                    println!("Stopped running bot {run_id} for user {} (machine {} from plugin {})", run.user_id, run.machine_name, run.plugin_id);
                    todo!("stop the machine from the scheduler")
                }
                Ok(())
            } else {
                Err(failed!("Could not start a transaction"))
            }
        } else {
            Err(failed!("No DB Connection"))
        }
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
                println!("Received {sym}{t:#?}");
                // TODO: propagate data to the corresponding machines
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
