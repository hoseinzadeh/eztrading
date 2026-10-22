//! Studio as a client of the server cluster: it talks to `srvman` (the service manager) to put a
//! strategy on the cluster (upload, compile, load, launch) and to list and stop bots.
//!
//! The wire protocol is the one the CLI uses (`servers/com`). Requests are answered at once with a
//! job id, and the result of a job (a compile, a launch) arrives later as a pushed packet, which a
//! receiver thread stores so that a request can wait for it.

use com::net::{
    BotInfo, CodeSrvRep, ConnectionInternal, Handshake, JobSrvRep, Packet, SrvManListReq, SrvManRep, SrvManReq, Upload,
};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
/// Compiling a plugin takes a while (a C++ build).
const BUILD_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_TASKS: usize = 50;

type Jobs = Arc<(Mutex<HashMap<u64, SrvManRep>>, Condvar)>;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

struct Link {
    conn: ConnectionInternal,
    alive: Arc<AtomicBool>,
    _threads: Vec<std::thread::JoinHandle<com::status::Status>>,
}

impl Drop for Link {
    fn drop(&mut self) {
        let _ = self.conn.disconnect();
        self.alive.store(false, Ordering::Relaxed);
    }
}

pub struct Cluster {
    addr: Option<String>,
    link: Mutex<Option<Link>>,
    jobs: Jobs,
    tasks: Mutex<BTreeMap<u64, Arc<Mutex<Task>>>>,
    next_task: AtomicU64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Step {
    pub label: String,
    /// `running`, `ok` or `failed`
    pub status: &'static str,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Task {
    pub id: u64,
    pub name: String,
    pub steps: Vec<Step>,
    pub done: bool,
    pub error: Option<String>,
    pub bot: Option<u32>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct DeployRequest {
    pub user: u32,
    pub name: String,
    pub source: String,
    /// The machine to launch (as named in the strategy, e.g. `Threshold`)
    pub machine: String,
    /// Parameter id -> text value
    #[serde(default)]
    pub params: BTreeMap<String, String>,
}

impl Cluster {
    pub fn new(addr: Option<String>) -> Arc<Cluster> {
        Arc::new(Cluster { addr, link: Mutex::new(None), jobs: Default::default(), tasks: Mutex::new(BTreeMap::new()), next_task: AtomicU64::new(1) })
    }

    pub fn configured(&self) -> bool {
        self.addr.is_some()
    }

    pub fn addr(&self) -> Option<&str> {
        self.addr.as_deref()
    }

    fn connect(&self) -> Result<Link, String> {
        let addr = self.addr.clone().ok_or("no srvman address is configured (start Studio with --srvman HOST:PORT)")?;
        let alive = Arc::new(AtomicBool::new(true));
        let (mut threads, conn) = ConnectionInternal::connect(&addr, None, alive.clone(), None).map_err(|e| format!("cannot reach srvman at {addr}: {e}"))?;
        // The server introduces itself first; answer as a client (the same handshake the CLI does).
        let hs = Handshake::from(conn.recv().map_err(|e| format!("no handshake from {addr}: {e}"))?);
        if !matches!(hs, Handshake::SrvMan(_)) {
            return Err(format!("{addr} is not a srvman (it says it is {hs:?})"));
        }
        conn.send(Handshake::CLI(hs.secret())).map_err(|e| format!("handshake with {addr} failed: {e}"))?;
        let jobs = self.jobs.clone();
        threads.push(conn.start_async_recv(move |p: Packet, _| {
            let rep = SrvManRep::from(p);
            let id = match &rep {
                SrvManRep::CodeSrv(id, _) | SrvManRep::JobSrv(id, _) => Some(*id),
                _ => None,
            };
            if let Some(id) = id {
                lock(&jobs.0).insert(id, rep);
                jobs.1.notify_all();
            }
        }));
        Ok(Link { conn, alive, _threads: threads })
    }

    /// One request/answer exchange. The protocol has no request ids, so an answer of the wrong
    /// kind (a stray or late reply to an earlier request) is skipped rather than believed.
    /// Reconnects once if the link has gone away.
    fn request(&self, req: SrvManReq) -> Result<SrvManRep, String> {
        let expect: fn(&SrvManRep) -> bool = match &req {
            SrvManReq::List(SrvManListReq::Clients) => |r| matches!(r, SrvManRep::Clients(_)),
            SrvManReq::List(SrvManListReq::Users) => |r| matches!(r, SrvManRep::Users(_)),
            SrvManReq::List(SrvManListReq::Bots) => |r| matches!(r, SrvManRep::Bots(_)),
            SrvManReq::List(SrvManListReq::Plugins) => |r| matches!(r, SrvManRep::JobSrv(_, JobSrvRep::AllPlugins(_))),
            SrvManReq::Upload(_) | SrvManReq::Build(..) | SrvManReq::LoadPlugin(..) | SrvManReq::Launch(..) => |r| matches!(r, SrvManRep::Submitted(_)),
            SrvManReq::Stop(_) => |r| matches!(r, SrvManRep::JobSrv(..)),
            SrvManReq::Subscribe(..) | SrvManReq::Unsubscribe(..) => |r| matches!(r, SrvManRep::Successful),
            _ => |_| true,
        };
        let mut guard = lock(&self.link);
        for attempt in 0..2 {
            if guard.as_ref().map_or(true, |l| !l.conn.connected()) {
                *guard = Some(self.connect()?);
            }
            let link = guard.as_ref().unwrap();
            // the reply queue has no timeout of its own: give up through a watchdog
            let done = Arc::new(AtomicBool::new(false));
            let (d2, alive) = (done.clone(), link.alive.clone());
            std::thread::spawn(move || {
                let start = Instant::now();
                while !d2.load(Ordering::Relaxed) {
                    if start.elapsed() > REQUEST_TIMEOUT {
                        alive.store(false, Ordering::Relaxed);
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            });
            let outcome = (|| -> Result<SrvManRep, Option<String>> {
                link.conn.send(req.clone()).map_err(|_| None)?;
                loop {
                    let rep = SrvManRep::from(link.conn.recv().map_err(|_| None)?);
                    match rep {
                        SrvManRep::Error(e) => return Err(Some(e.to_string())),
                        rep if expect(&rep) => return Ok(rep),
                        _ => continue, // not the answer to this request
                    }
                }
            })();
            done.store(true, Ordering::Relaxed);
            match outcome {
                Ok(rep) => return Ok(rep),
                Err(Some(message)) => return Err(message),
                Err(None) if attempt == 0 => *guard = None, // a stale link: reconnect and try once more
                Err(None) => return Err("srvman did not answer".into()),
            }
        }
        Err("srvman did not answer".into())
    }

    fn wait_job(&self, id: u64, timeout: Duration) -> Result<SrvManRep, String> {
        let start = Instant::now();
        let mut jobs = lock(&self.jobs.0);
        loop {
            if let Some(rep) = jobs.remove(&id) {
                return Ok(rep);
            }
            let left = timeout.checked_sub(start.elapsed()).ok_or_else(|| format!("job {id} did not finish in time"))?;
            jobs = self.jobs.1.wait_timeout(jobs, left.min(Duration::from_millis(500))).unwrap_or_else(|p| p.into_inner()).0;
        }
    }

    fn submit(&self, req: SrvManReq) -> Result<Vec<u64>, String> {
        match self.request(req)? {
            SrvManRep::Submitted(ids) => Ok(ids),
            other => Err(format!("unexpected answer from srvman: {other:?}")),
        }
    }

    // ---- reading -------------------------------------------------------------------

    pub fn status(&self) -> Value {
        if !self.configured() {
            return json!({ "configured": false });
        }
        let servers = match self.request(SrvManReq::List(SrvManListReq::Clients)) {
            Ok(SrvManRep::Clients(c)) => c,
            Ok(_) => vec![],
            Err(e) => return json!({ "configured": true, "connected": false, "addr": self.addr, "error": e }),
        };
        let mut problems: Vec<String> = vec![];
        let users = match self.request(SrvManReq::List(SrvManListReq::Users)) {
            Ok(SrvManRep::Users(u)) => serde_json::to_value(u).unwrap_or(Value::Null),
            Ok(other) => {
                problems.push(format!("users: unexpected answer {other:?}"));
                json!([])
            }
            Err(e) => {
                problems.push(format!("users: {e}"));
                json!([])
            }
        };
        let plugins = match self.request(SrvManReq::List(SrvManListReq::Plugins)) {
            Ok(SrvManRep::JobSrv(_, JobSrvRep::AllPlugins(p))) => {
                let mut list: Vec<_> = p.into_iter().collect();
                list.sort_by_key(|(id, _)| *id);
                list.into_iter().map(|(id, p)| plugin_view(id, &p)).collect::<Vec<_>>()
            }
            Ok(other) => {
                problems.push(format!("plugins: unexpected answer {other:?}"));
                vec![]
            }
            Err(e) => {
                problems.push(format!("plugins: {e}"));
                vec![]
            }
        };
        let bots: Vec<BotInfo> = match self.request(SrvManReq::List(SrvManListReq::Bots)) {
            Ok(SrvManRep::Bots(b)) => b,
            Ok(other) => {
                problems.push(format!("bots: unexpected answer {other:?}"));
                vec![]
            }
            Err(e) => {
                problems.push(format!("bots: {e}"));
                vec![]
            }
        };
        json!({ "configured": true, "connected": true, "addr": self.addr, "servers": servers, "users": users, "plugins": plugins, "bots": bots, "problems": problems })
    }

    pub fn stop_bot(&self, id: u32) -> Result<(), String> {
        match self.request(SrvManReq::Stop(id))? {
            SrvManRep::JobSrv(_, JobSrvRep::Successful) => Ok(()),
            SrvManRep::JobSrv(_, JobSrvRep::Error(e)) => Err(e.to_string()),
            other => Err(format!("unexpected answer from srvman: {other:?}")),
        }
    }

    // ---- deploying -------------------------------------------------------------------

    pub fn tasks(&self) -> Vec<Task> {
        lock(&self.tasks).values().rev().map(|t| lock(t).clone()).collect()
    }

    pub fn task(&self, id: u64) -> Option<Task> {
        lock(&self.tasks).get(&id).map(|t| lock(t).clone())
    }

    /// Starts the pipeline in the background and returns the task to poll.
    pub fn deploy(self: &Arc<Self>, req: DeployRequest) -> Result<u64, String> {
        if !self.configured() {
            return Err("no srvman address is configured".into());
        }
        let name = req.name.trim().to_owned();
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') || name.len() > 60 {
            return Err("the plugin name needs letters, digits, `-` or `_` (up to 60)".into());
        }
        if req.source.len() > 256 * 1024 {
            return Err("the strategy is too large".into());
        }
        // Inline C++ is compiled and run natively on the cluster, and Studio has no login.
        if req.source.contains("{%") {
            return Err("strategies with inline C++ ({% ... %}) are not accepted from Studio: they would run as native code on the cluster. Upload them with the CLI if you trust them".into());
        }
        let id = self.next_task.fetch_add(1, Ordering::SeqCst);
        let task = Arc::new(Mutex::new(Task { id, name: name.clone(), steps: vec![], done: false, error: None, bot: None }));
        {
            let mut tasks = lock(&self.tasks);
            tasks.insert(id, task.clone());
            while tasks.len() > MAX_TASKS {
                let first = *tasks.keys().next().unwrap();
                tasks.remove(&first);
            }
        }
        let me = self.clone();
        std::thread::Builder::new()
            .name(format!("cluster-deploy-{id}"))
            .spawn(move || {
                let r = me.run_deploy(&task, DeployRequest { name, ..req });
                let mut t = lock(&task);
                t.done = true;
                if let Err(e) = r {
                    if let Some(last) = t.steps.last_mut() {
                        if last.status == "running" {
                            last.status = "failed";
                            last.detail = e.clone();
                        }
                    }
                    t.error = Some(e);
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(id)
    }

    fn run_deploy(&self, task: &Arc<Mutex<Task>>, req: DeployRequest) -> Result<(), String> {
        let step = |label: &str| {
            lock(task).steps.push(Step { label: label.to_owned(), status: "running", detail: String::new() });
        };
        let ok = |detail: String| {
            if let Some(s) = lock(task).steps.last_mut() {
                s.status = "ok";
                s.detail = detail;
            }
        };

        step("Checking the cluster");
        let existing: Option<u32> = match self.request(SrvManReq::List(SrvManListReq::Plugins))? {
            SrvManRep::JobSrv(_, JobSrvRep::AllPlugins(p)) => p.iter().find(|(_, e)| e.name == req.name && e.owner_id == req.user).map(|(id, _)| *id),
            _ => None,
        };
        let running: Vec<u32> = match self.request(SrvManReq::List(SrvManListReq::Bots))? {
            SrvManRep::Bots(b) => b.into_iter().filter(|b| Some(b.plugin_id) == existing && b.user_id == req.user).map(|b| b.id).collect(),
            _ => vec![],
        };
        ok(match existing {
            Some(id) => format!("plugin {id} exists; {} running bot(s) of yours will be replaced", running.len()),
            None => "a new plugin".into(),
        });

        step("Uploading the source");
        let ids = self.submit(SrvManReq::Upload(Upload { id: existing, user: Some(req.user), name: Some(req.name.clone()), source: Some(req.source.clone()), version: Some("1.0.0".into()), price: None }))?;
        let plugin_id = match self.wait_job(ids[0], REQUEST_TIMEOUT)? {
            SrvManRep::CodeSrv(_, CodeSrvRep::Successful(id, _, _)) => id,
            SrvManRep::CodeSrv(_, CodeSrvRep::Error(e)) => return Err(e.to_string()),
            other => return Err(format!("unexpected answer: {other:?}")),
        };
        ok(format!("plugin {plugin_id}"));

        step("Compiling on a code server (this takes a minute)");
        let ids = self.submit(SrvManReq::Build(plugin_id, false, None))?;
        let machines = match self.wait_job(ids[0], BUILD_TIMEOUT)? {
            SrvManRep::CodeSrv(_, CodeSrvRep::Successful(_, _, m)) => m,
            SrvManRep::CodeSrv(_, CodeSrvRep::Error(e)) => return Err(e.to_string()),
            other => return Err(format!("unexpected answer: {other:?}")),
        };
        ok(format!("{} machine(s): {}", machines.len(), machines.iter().map(|m| short_name(&m.name)).collect::<Vec<_>>().join(", ")));

        let mut machine = machines
            .into_iter()
            .find(|m| short_name(&m.name) == req.machine)
            .ok_or_else(|| format!("the strategy has no machine named `{}`", req.machine))?;
        for (k, v) in &req.params {
            match machine.params.iter_mut().find(|p| &p.id == k) {
                Some(p) => p.set_value(v).map_err(|e| format!("parameter `{k}`: {e}"))?,
                None => return Err(format!("the machine has no parameter `{k}`")),
            }
        }

        if !running.is_empty() {
            step("Stopping the previous version");
            for id in &running {
                self.stop_bot(*id)?;
            }
            ok(format!("stopped {} bot(s)", running.len()));
        }

        step("Allowing the user to run it");
        let expire = com::chrono::Utc::now().naive_utc().date() + com::chrono::Duration::days(365);
        // a subscription that already exists is fine
        let _ = self.request(SrvManReq::Subscribe(req.user, plugin_id, false, expire));
        ok("subscribed".into());

        step("Loading it on the job servers");
        let ids = self.submit(SrvManReq::LoadPlugin(plugin_id, true))?;
        let mut loaded = 0;
        for id in &ids {
            match self.wait_job(*id, BUILD_TIMEOUT)? {
                SrvManRep::JobSrv(_, JobSrvRep::Plugin(..)) => loaded += 1,
                SrvManRep::JobSrv(_, JobSrvRep::Error(e)) => return Err(e.to_string()),
                other => return Err(format!("unexpected answer: {other:?}")),
            }
        }
        ok(format!("loaded on {loaded} server(s)"));

        step("Starting the bot");
        let ids = self.submit(SrvManReq::Launch(req.user, machine))?;
        match self.wait_job(ids[0], REQUEST_TIMEOUT)? {
            SrvManRep::JobSrv(_, JobSrvRep::Successful) => {}
            SrvManRep::JobSrv(_, JobSrvRep::Error(e)) => return Err(e.to_string()),
            other => return Err(format!("unexpected answer: {other:?}")),
        }
        let bot = match self.request(SrvManReq::List(SrvManListReq::Bots))? {
            SrvManRep::Bots(b) => b.into_iter().filter(|b| b.plugin_id == plugin_id && b.user_id == req.user).map(|b| b.id).max(),
            _ => None,
        };
        lock(task).bot = bot;
        ok(match bot {
            Some(b) => format!("bot {b} is running"),
            None => "started".into(),
        });
        Ok(())
    }
}

fn short_name(machine: &str) -> String {
    machine.rsplit("::").next().unwrap_or(machine).to_owned()
}

fn plugin_view(id: u32, p: &com::net::PluginDBEntry) -> Value {
    let machines: Vec<Value> = p
        .machines
        .iter()
        .flatten()
        .map(|m| {
            let params: Vec<Value> = m
                .params
                .iter()
                .map(|q| {
                    let text = q.value_text();
                    json!({ "id": q.id, "name": q.name, "type": q.r#type, "value": text, "editable": text.is_some() })
                })
                .collect();
            json!({ "name": short_name(&m.name), "params": params })
        })
        .collect();
    json!({ "id": id, "name": p.name, "owner_id": p.owner_id, "owner": p.owner_name, "version": p.version, "published": p.published, "machines": machines })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_is_sent_when_no_cluster_is_configured() {
        let c = Cluster::new(None);
        assert!(!c.configured());
        assert_eq!(c.status()["configured"], false);
        let req = |source: &str| DeployRequest { user: 1, name: "x".into(), source: source.into(), machine: "M".into(), params: Default::default() };
        assert!(c.deploy(req("machine M {}")).unwrap_err().contains("srvman"));
    }

    #[test]
    fn names_and_inline_cpp_are_checked_before_anything_is_sent() {
        let c = Cluster::new(Some("127.0.0.1:1".into()));
        let req = |name: &str, source: &str| DeployRequest { user: 1, name: name.into(), source: source.into(), machine: "M".into(), params: Default::default() };
        assert!(c.deploy(req("bad name!", "machine M {}")).unwrap_err().contains("letters, digits"));
        assert!(c.deploy(req("ok", "machine M { {% system(\"x\"); %} }")).unwrap_err().contains("inline C++"));
        assert!(c.deploy(req("ok", &"x".repeat(300_000))).unwrap_err().contains("too large"));
        // an unreachable srvman is reported by the task, not by a hang
        let id = c.deploy(req("ok", "machine M {}")).unwrap();
        for _ in 0..100 {
            if c.task(id).map_or(false, |t| t.done) {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let t = c.task(id).unwrap();
        assert!(t.done && t.error.as_deref().unwrap_or("").contains("cannot reach srvman"), "{t:?}");
    }
}
