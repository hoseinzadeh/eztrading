//! The HTTP API and the embedded web UI of Strategy Studio.
//!
//! Local-first by design: it binds to loopback by default, answers only to
//! known `Host` headers (DNS-rebinding protection), accepts mutations only as
//! `application/json` (so a foreign web page cannot trigger them without a CORS
//! preflight, which is never granted), and only touches `*.sm` and
//! `*.test.json` files directly inside its workspace directory.

use crate::analyze::{analyze, simulation_blockers};
use crate::ast::show;
use crate::diagram::diagram;
use crate::live::{self, Cursor, DeployRequest, LiveError};
use crate::market::{self, Generator, Market};
use crate::model::{self, Diagnostic, Severity};
use crate::sim::{self, Config};
use crate::testing;
use actix_web::http::header;
use actix_web::{guard, web, HttpResponse, ResponseError};
use ezt_agent::agent::{self, Context, Task};
use ezt_agent::compiler::{Checker, Validator};
use ezt_agent::llm::Llm;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

pub const INDEX_HTML: &str = include_str!("../assets/index.html");
pub const APP_JS: &str = include_str!("../assets/app.js");
pub const APP_CSS: &str = include_str!("../assets/app.css");
pub const SML_JS: &str = include_str!("../assets/sml-language.js");

const MAX_FILE_BYTES: usize = 256 * 1024;
const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;

pub type SharedLlm = Arc<dyn Llm + Send + Sync>;

pub struct AppState {
    pub dir: PathBuf,
    pub checker: Arc<Checker>,
    pub llm: Option<SharedLlm>,
    pub llm_note: String,
    pub model: String,
    pub stdlib: String,
    pub allowed_hosts: Vec<String>,
    pub live: Arc<live::Manager>,
    pub cluster: Arc<crate::cluster::Cluster>,
}

#[derive(Debug)]
pub struct ApiError {
    status: actix_web::http::StatusCode,
    message: String,
}

impl ApiError {
    fn bad(message: impl Into<String>) -> Self {
        Self { status: actix_web::http::StatusCode::BAD_REQUEST, message: message.into() }
    }
    fn not_found(message: impl Into<String>) -> Self {
        Self { status: actix_web::http::StatusCode::NOT_FOUND, message: message.into() }
    }
    fn conflict(message: impl Into<String>) -> Self {
        Self { status: actix_web::http::StatusCode::CONFLICT, message: message.into() }
    }
    fn internal(message: impl Into<String>) -> Self {
        Self { status: actix_web::http::StatusCode::INTERNAL_SERVER_ERROR, message: message.into() }
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl ResponseError for ApiError {
    fn status_code(&self) -> actix_web::http::StatusCode {
        self.status
    }
    fn error_response(&self) -> HttpResponse {
        HttpResponse::build(self.status).json(json!({ "error": self.message }))
    }
}

type ApiResult = Result<HttpResponse, ApiError>;

pub fn host_allowed(host_header: Option<&str>, allowed: &[String]) -> bool {
    let Some(host) = host_header else { return false };
    // Strip the port; keep IPv6 brackets intact.
    let name = if let Some(rest) = host.strip_prefix('[') {
        rest.split(']').next().map(|h| format!("[{h}]")).unwrap_or_default()
    } else {
        host.split(':').next().unwrap_or("").to_owned()
    };
    let name = name.to_lowercase();
    matches!(name.as_str(), "localhost" | "127.0.0.1" | "[::1]") || allowed.iter().any(|a| a.eq_ignore_ascii_case(&name))
}

/// A request that carries an `Origin` must come from one of our own origins.
/// (Browsers add `Origin` to cross-site writes; same-origin GETs may omit it.)
pub fn origin_allowed(origin: Option<&str>, allowed: &[String]) -> bool {
    match origin {
        None => true,
        Some(o) => {
            let rest = o.strip_prefix("http://").or_else(|| o.strip_prefix("https://"));
            rest.map_or(false, |r| host_allowed(Some(r.split('/').next().unwrap_or("")), allowed))
        }
    }
}

pub fn configure(cfg: &mut web::ServiceConfig, state: web::Data<AppState>) {
    let allowed = state.allowed_hosts.clone();
    cfg.app_data(state)
        .app_data(web::JsonConfig::default().limit(MAX_BODY_BYTES).error_handler(|err, _| ApiError::bad(err.to_string()).into()))
        .service(
            web::scope("")
                .guard(guard::fn_guard(move |ctx| {
                    let h = ctx.head().headers();
                    host_allowed(h.get(header::HOST).and_then(|v| v.to_str().ok()), &allowed)
                        && origin_allowed(h.get(header::ORIGIN).and_then(|v| v.to_str().ok()), &allowed)
                }))
                .route("/", web::get().to(|| async { page("text/html; charset=utf-8", INDEX_HTML) }))
                .route("/app.js", web::get().to(|| async { page("text/javascript; charset=utf-8", APP_JS) }))
                .route("/app.css", web::get().to(|| async { page("text/css; charset=utf-8", APP_CSS) }))
                .route("/sml-language.js", web::get().to(|| async { page("text/javascript; charset=utf-8", SML_JS) }))
                .route("/favicon.ico", web::get().to(|| async { HttpResponse::NoContent().finish() }))
                .route("/api/health", web::get().to(health))
                .route("/api/files", web::get().to(list_files))
                .route("/api/files/{name}", web::get().to(read_file))
                .route("/api/files/{name}", web::put().to(write_file))
                .route("/api/analyze", web::post().to(analyze_source))
                .route("/api/compile", web::post().to(compile_source))
                .route("/api/backtest", web::post().to(run_backtest))
                .route("/api/test", web::post().to(run_tests))
                .route("/api/cluster", web::get().to(cluster_status))
                .route("/api/cluster/deploy", web::post().to(cluster_deploy))
                .route("/api/cluster/tasks", web::get().to(cluster_tasks))
                .route("/api/cluster/tasks/{id}", web::get().to(cluster_task))
                .route("/api/cluster/bots/{id}/stop", web::post().to(cluster_stop))
                .route("/api/brokers", web::get().to(brokers_list))
                .route("/api/brokers/kill", web::post().to(brokers_kill))
                .route("/api/brokers/{name}/test", web::post().to(brokers_test))
                .route("/api/live", web::get().to(live_list))
                .route("/api/live", web::post().to(live_deploy))
                .route("/api/live/emergency-stop", web::post().to(live_emergency))
                .route("/api/live/{id}", web::get().to(live_detail))
                .route("/api/live/{id}", web::delete().to(live_delete))
                .route("/api/live/{id}/source", web::get().to(live_source))
                .route("/api/live/{id}/limits", web::put().to(live_limits))
                .route("/api/live/{id}/{action}", web::post().to(live_action))
                .route("/api/ai/status", web::get().to(ai_status))
                .route("/api/ai/{task}", web::post().to(ai_task)),
        );
}

fn page(content_type: &'static str, body: &'static str) -> HttpResponse {
    HttpResponse::Ok()
        .insert_header((header::CONTENT_TYPE, content_type))
        .insert_header((header::CACHE_CONTROL, "no-cache"))
        .insert_header(("X-Content-Type-Options", "nosniff"))
        .body(body)
}

async fn health(state: web::Data<AppState>) -> HttpResponse {
    HttpResponse::Ok().json(json!({ "ok": true, "ai": state.llm.is_some() }))
}

// ---- files ---------------------------------------------------------------

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('.')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        && (name.ends_with(".sm") || name.ends_with(".test.json"))
        && !name.contains("..")
}

fn file_path(state: &AppState, name: &str) -> Result<PathBuf, ApiError> {
    if !valid_name(name) {
        return Err(ApiError::bad("file names may use letters, digits, `_`, `-` and `.`, and must end in .sm or .test.json"));
    }
    Ok(state.dir.join(name))
}

async fn list_files(state: web::Data<AppState>) -> ApiResult {
    let mut files = vec![];
    let entries = std::fs::read_dir(&state.dir).map_err(|e| ApiError::internal(format!("cannot read workspace: {e}")))?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let meta = entry.metadata().ok();
        if valid_name(&name) && meta.as_ref().map_or(false, |m| m.is_file()) {
            let kind = if name.ends_with(".sm") { "strategy" } else { "tests" };
            files.push(json!({ "name": name, "kind": kind, "size": meta.map_or(0, |m| m.len()) }));
        }
    }
    files.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    Ok(HttpResponse::Ok().json(json!({ "files": files })))
}

async fn read_file(state: web::Data<AppState>, name: web::Path<String>) -> ApiResult {
    let path = file_path(&state, &name)?;
    match std::fs::read_to_string(&path) {
        Ok(content) => Ok(HttpResponse::Ok().json(json!({ "name": *name, "content": content }))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(ApiError::not_found("no such file")),
        Err(e) => Err(ApiError::internal(format!("cannot read file: {e}"))),
    }
}

#[derive(Deserialize)]
struct WriteReq {
    content: String,
}

async fn write_file(state: web::Data<AppState>, name: web::Path<String>, body: web::Json<WriteReq>) -> ApiResult {
    let path = file_path(&state, &name)?;
    if body.content.len() > MAX_FILE_BYTES {
        return Err(ApiError::bad(format!("files are limited to {} KiB", MAX_FILE_BYTES / 1024)));
    }
    let tmp = state.dir.join(format!(".{name}.tmp"));
    std::fs::write(&tmp, &body.content).and_then(|_| std::fs::rename(&tmp, &path)).map_err(|e| ApiError::internal(format!("cannot write file: {e}")))?;
    Ok(HttpResponse::Ok().json(json!({ "ok": true })))
}

// ---- analysis ------------------------------------------------------------

#[derive(Deserialize)]
struct SourceReq {
    source: String,
    #[serde(default)]
    machine: Option<String>,
}

fn params_json(strategy: &model::Strategy, machine: Option<&str>) -> Value {
    let Some(m) = strategy.machine(machine).or_else(|| strategy.machine(None)) else { return json!([]) };
    let items: Vec<Value> = m
        .params
        .iter()
        .map(|p| {
            let default = p.default.as_ref().map(show);
            let all_symbols = p.ty.starts_with("Vec") && matches!(&p.default, None) || default.as_deref() == Some("{}");
            json!({
                "name": p.name,
                "type": p.ty,
                "label": p.label,
                "default": if all_symbols { Value::Null } else { json!(default) },
                "hint": if all_symbols { json!("all symbols in the data") } else { Value::Null },
                "options": p.options.iter().map(|(v, l)| json!({ "value": show(v), "label": l })).collect::<Vec<_>>(),
                "required": p.default.is_none() && !all_symbols && p.ty != "Account" && p.ty != "Symbol",
            })
        })
        .collect();
    Value::Array(items)
}

async fn analyze_source(body: web::Json<SourceReq>) -> ApiResult {
    let SourceReq { source, machine } = body.into_inner();
    if source.len() > MAX_FILE_BYTES {
        return Err(ApiError::bad("source is too large"));
    }
    let resp = web::block(move || match model::parse(&source) {
        Err(d) => json!({ "ok": false, "diagnostics": [d], "diagram": null, "params": [], "machines": [], "sim_notes": [] }),
        Ok(strategy) => {
            let mut diagnostics = analyze(&strategy);
            diagnostics.extend(strategy.notes.iter().cloned());
            let d = diagram(&strategy, machine.as_deref());
            json!({
                "ok": true,
                "diagnostics": diagnostics,
                "diagram": d,
                "params": params_json(&strategy, machine.as_deref()),
                "machines": strategy.machines.iter().map(|m| m.name.clone()).collect::<Vec<_>>(),
                "sim_notes": simulation_blockers(&strategy, machine.as_deref().filter(|n| strategy.machine(Some(n)).is_some())),
            })
        }
    })
    .await
    .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(HttpResponse::Ok().json(resp))
}

/// Parses the compiler's `strategy.sm:(row,col): message` lines into positioned diagnostics.
fn compiler_diagnostics(lines: &[String]) -> Vec<Diagnostic> {
    lines
        .iter()
        .filter(|l| !l.starts_with("lint:")) // positioned equivalents come from the analysis
        .map(|l| {
            let rest = l.strip_prefix("strategy.sm:").unwrap_or(l);
            if let Some(inner) = rest.strip_prefix('(') {
                if let Some((pos, msg)) = inner.split_once("):") {
                    if let Some((r, c)) = pos.split_once(',') {
                        if let (Ok(r), Ok(c)) = (r.trim().parse(), c.trim().parse()) {
                            return Diagnostic::new(r, c, Severity::Error, msg.trim());
                        }
                    }
                }
            }
            Diagnostic::new(1, 1, Severity::Error, rest.trim())
        })
        .collect()
}

async fn compile_source(state: web::Data<AppState>, body: web::Json<SourceReq>) -> ApiResult {
    let source = body.into_inner().source;
    if source.len() > MAX_FILE_BYTES {
        return Err(ApiError::bad("source is too large"));
    }
    let checker = state.checker.clone();
    let report = web::block(move || checker.check(&source)).await.map_err(|e| ApiError::internal(e.to_string()))?;
    let mut diagnostics = compiler_diagnostics(&report.diagnostics);
    for d in &mut diagnostics {
        if d.message.contains("cannot run compiler") {
            d.message.push_str(" - build it with `cargo build -p compile` (or pass --compiler)");
        }
    }
    // `ok` reflects the compiler only; lints are shown by the analysis.
    let ok = diagnostics.is_empty() && !report.diagnostics.iter().any(|l| l.starts_with("internal error"));
    Ok(HttpResponse::Ok().json(json!({ "ok": ok, "diagnostics": diagnostics, "text": report.render() })))
}

// ---- backtest and tests --------------------------------------------------

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum DataSpec {
    Generate {
        #[serde(default = "default_symbol")]
        symbol: String,
        #[serde(default)]
        generator: Generator,
    },
    Csv {
        #[serde(default = "default_symbol")]
        symbol: String,
        csv: String,
    },
}

fn default_symbol() -> String {
    "TEST".into()
}

fn default_bar_seconds() -> f64 {
    86_400.0
}

fn default_cash() -> f64 {
    100_000.0
}

#[derive(Deserialize)]
struct BacktestReq {
    source: String,
    #[serde(default)]
    machine: Option<String>,
    #[serde(default)]
    params: BTreeMap<String, Value>,
    data: DataSpec,
    #[serde(default = "default_bar_seconds")]
    bar_seconds: f64,
    #[serde(default = "default_cash")]
    initial_cash: f64,
    #[serde(default)]
    fee_bps: f64,
}

fn fail(message: impl Into<String>) -> Value {
    json!({ "ok": false, "error": { "message": message.into() } })
}

async fn run_backtest(body: web::Json<BacktestReq>) -> ApiResult {
    let req = body.into_inner();
    if req.source.len() > MAX_FILE_BYTES {
        return Err(ApiError::bad("source is too large"));
    }
    let resp = web::block(move || -> Value {
        let strategy = match model::parse(&req.source) {
            Ok(s) => s,
            Err(d) => return json!({ "ok": false, "error": { "line": d.line, "message": d.message } }),
        };
        let (symbol, bars) = match &req.data {
            DataSpec::Generate { symbol, generator } => match market::generate(generator, req.bar_seconds) {
                Ok(b) => (symbol.clone(), b),
                Err(e) => return fail(e),
            },
            DataSpec::Csv { symbol, csv } => match market::parse_csv(csv, req.bar_seconds) {
                Ok(b) => (symbol.clone(), b),
                Err(e) => return fail(e),
            },
        };
        let prices: Vec<f64> = bars.iter().map(|b| b.close).collect();
        let market = Market::single(&symbol, bars, req.bar_seconds);
        let cfg = Config { machine: req.machine.clone(), params: req.params.clone(), initial_cash: req.initial_cash, fee_bps: req.fee_bps, ..Default::default() };
        match sim::backtest(&strategy, &market, &cfg) {
            Err(e) => json!({ "ok": false, "error": { "line": e.line, "bar": e.bar, "message": e.message } }),
            Ok(b) => {
                let golden = testing::golden("captured from backtest", &symbol, prices, req.params.clone(), req.machine.clone(), &b);
                json!({ "ok": true, "backtest": b, "golden": golden })
            }
        }
    })
    .await
    .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(HttpResponse::Ok().json(resp))
}

#[derive(Deserialize)]
struct TestReq {
    source: String,
    /// The text of a `.test.json` file.
    tests: String,
}

async fn run_tests(body: web::Json<TestReq>) -> ApiResult {
    let req = body.into_inner();
    let resp = web::block(move || -> Value {
        let strategy = match model::parse(&req.source) {
            Ok(s) => s,
            Err(d) => return json!({ "ok": false, "error": format!("the strategy has a syntax error on line {}: {}", d.line, d.message) }),
        };
        let file = match testing::parse_tests(&req.tests) {
            Ok(f) => f,
            Err(e) => return json!({ "ok": false, "error": e }),
        };
        let results = testing::run_tests(&strategy, &file);
        let passed = results.iter().filter(|r| r.passed).count();
        json!({ "ok": true, "results": results, "passed": passed, "failed": results.len() - passed })
    })
    .await
    .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(HttpResponse::Ok().json(resp))
}

// ---- AI --------------------------------------------------------------------

async fn ai_status(state: web::Data<AppState>) -> HttpResponse {
    HttpResponse::Ok().json(json!({ "enabled": state.llm.is_some(), "model": state.model, "note": state.llm_note }))
}

#[derive(Deserialize, Default)]
struct AiReq {
    #[serde(default)]
    prompt: String,
    #[serde(default)]
    source: String,
}

async fn ai_task(state: web::Data<AppState>, task: web::Path<String>, body: web::Json<AiReq>) -> ApiResult {
    let Some(llm) = state.llm.clone() else {
        return Ok(HttpResponse::Ok().json(json!({ "ok": false, "error": state.llm_note.clone() })));
    };
    let task = task.into_inner();
    let req = body.into_inner();
    if req.prompt.len() > 8_000 || req.source.len() > MAX_FILE_BYTES {
        return Err(ApiError::bad("the request is too large"));
    }
    let checker = state.checker.clone();
    let stdlib = state.stdlib.clone();
    let resp = web::block(move || -> Result<Value, ApiError> {
        let cfg = agent::Config { max_turns: 12, allow_cpp: false, verbose: false };
        let ctx = Context { llm: &*llm, validator: &*checker, stdlib: &stdlib };
        let diagnostics;
        let t = match task.as_str() {
            "generate" => {
                if req.prompt.trim().is_empty() {
                    return Err(ApiError::bad("describe the strategy you want"));
                }
                Task::Generate(&req.prompt)
            }
            "fix" => {
                diagnostics = checker.check(&req.source).render();
                Task::Fix { source: &req.source, diagnostics: &diagnostics }
            }
            "explain" => Task::Explain(&req.source),
            other => return Err(ApiError::not_found(format!("unknown AI task `{other}`"))),
        };
        Ok(match agent::run(&ctx, t, &cfg) {
            Ok(o) => json!({
                "ok": true,
                "source": o.source,
                "summary": o.summary,
                "usage": { "turns": o.turns, "checks": o.checks, "input_tokens": o.input_tokens, "output_tokens": o.output_tokens },
            }),
            Err(e) => json!({ "ok": false, "error": e }),
        })
    })
    .await
    .map_err(|e| ApiError::internal(e.to_string()))??;
    Ok(HttpResponse::Ok().json(resp))
}

// ---- live deployments -------------------------------------------------------

fn live_err(e: LiveError) -> ApiError {
    match e {
        LiveError::Invalid(m) => ApiError::bad(m),
        LiveError::NotFound => ApiError::not_found("no such deployment"),
        LiveError::Conflict(m) => ApiError::conflict(m),
        LiveError::Internal(m) => ApiError::internal(m),
    }
}

async fn live_list(state: web::Data<AppState>) -> ApiResult {
    Ok(HttpResponse::Ok().json(json!({
        "deployments": state.live.list(),
        "max_running": live::MAX_RUNNING,
        "max_deployments": live::MAX_DEPLOYMENTS,
        "max_bars": live::MAX_LIVE_BARS,
        "twelvedata": std::env::var("TWELVEDATA_API_KEY").map_or(false, |k| !k.trim().is_empty()),
        "kill_switch": state.live.brokers().kill_switch().is_tripped(),
    })))
}

// ---- the server cluster (srvman) -------------------------------------------

async fn cluster_status(state: web::Data<AppState>) -> ApiResult {
    let c = state.cluster.clone();
    let v = web::block(move || c.status()).await.map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(HttpResponse::Ok().json(v))
}

async fn cluster_deploy(state: web::Data<AppState>, body: web::Json<crate::cluster::DeployRequest>) -> ApiResult {
    let c = state.cluster.clone();
    let req = body.into_inner();
    let id = web::block(move || c.deploy(req)).await.map_err(|e| ApiError::internal(e.to_string()))?.map_err(ApiError::bad)?;
    Ok(HttpResponse::Ok().json(json!({ "task": id })))
}

async fn cluster_tasks(state: web::Data<AppState>) -> ApiResult {
    Ok(HttpResponse::Ok().json(json!({ "tasks": state.cluster.tasks() })))
}

async fn cluster_task(state: web::Data<AppState>, id: web::Path<u64>) -> ApiResult {
    match state.cluster.task(*id) {
        Some(t) => Ok(HttpResponse::Ok().json(t)),
        None => Err(ApiError::not_found("no such task")),
    }
}

async fn cluster_stop(state: web::Data<AppState>, id: web::Path<u32>) -> ApiResult {
    let c = state.cluster.clone();
    let id = id.into_inner();
    web::block(move || c.stop_bot(id)).await.map_err(|e| ApiError::internal(e.to_string()))?.map_err(ApiError::conflict)?;
    Ok(HttpResponse::Ok().json(json!({ "ok": true })))
}

// ---- brokerage connections -----------------------------------------------

async fn brokers_list(state: web::Data<AppState>) -> ApiResult {
    let brokers = state.live.brokers().clone();
    Ok(HttpResponse::Ok().json(json!({
        "brokers": brokers.list(),
        "kill_switch": brokers.kill_switch().is_tripped(),
        "allow_live": brokers.allow_live(),
    })))
}

#[derive(Deserialize)]
struct KillBody {
    on: bool,
}

/// Turns the global kill switch on (no deployment can buy) or off.
async fn brokers_kill(state: web::Data<AppState>, body: web::Json<KillBody>) -> ApiResult {
    let k = state.live.brokers().kill_switch();
    if body.on {
        k.trip();
    } else {
        k.reset();
    }
    Ok(HttpResponse::Ok().json(json!({ "kill_switch": k.is_tripped() })))
}

#[derive(Deserialize, Default)]
struct TestBody {
    #[serde(default)]
    symbol: Option<String>,
}

/// Read-only checks of a connection (account, positions, orders, optionally a quote).
async fn brokers_test(state: web::Data<AppState>, name: web::Path<String>, body: web::Json<TestBody>) -> ApiResult {
    let brokers = state.live.brokers().clone();
    let name = name.into_inner();
    if !crate::brokers::valid_name(&name) {
        return Err(ApiError::bad("invalid connection name"));
    }
    if let Some(sym) = body.symbol.as_deref().filter(|s| !s.is_empty()) {
        if !crate::feed::valid_symbol(sym) {
            return Err(ApiError::bad("invalid symbol"));
        }
    }
    let symbol = body.into_inner().symbol;
    let steps = web::block(move || brokers.test(&name, symbol.as_deref())).await.map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(HttpResponse::Ok().json(json!({ "ok": steps.iter().all(|s| s.ok), "steps": steps })))
}

/// Kill switch on, every brokerage deployment paused, working orders cancelled.
async fn live_emergency(state: web::Data<AppState>) -> ApiResult {
    let manager = state.live.clone();
    let paused = web::block(move || manager.emergency_stop()).await.map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(HttpResponse::Ok().json(json!({ "kill_switch": true, "paused": paused })))
}

async fn live_deploy(state: web::Data<AppState>, body: web::Json<DeployRequest>) -> ApiResult {
    let manager = state.live.clone();
    let req = body.into_inner();
    let summary = web::block(move || manager.deploy(req)).await.map_err(|e| ApiError::internal(e.to_string()))?.map_err(live_err)?;
    Ok(HttpResponse::Ok().json(summary))
}

async fn live_detail(state: web::Data<AppState>, id: web::Path<String>, cursor: web::Query<Cursor>) -> ApiResult {
    let dep = state.live.get(&id).map_err(live_err)?;
    Ok(HttpResponse::Ok().json(dep.detail(&cursor)))
}

async fn live_source(state: web::Data<AppState>, id: web::Path<String>) -> ApiResult {
    let (source, file, machine) = state.live.get(&id).map_err(live_err)?.source();
    Ok(HttpResponse::Ok().json(json!({ "source": source, "strategy_file": file, "machine": machine })))
}

#[derive(Deserialize, Default)]
struct ActionBody {
    /// For `stop`: close open positions first.
    #[serde(default)]
    flatten: bool,
}

async fn live_action(state: web::Data<AppState>, path: web::Path<(String, String)>, body: web::Json<ActionBody>) -> ApiResult {
    let (id, action) = path.into_inner();
    if !["pause", "resume", "stop", "flatten", "cancel-orders"].contains(&action.as_str()) {
        return Err(ApiError::not_found(format!("unknown action `{action}`")));
    }
    let manager = state.live.clone();
    let flatten = body.flatten;
    // Flatten, stop and cancel talk to the brokerage with blocking HTTP, which must not run on the async workers.
    let r = web::block(move || -> Result<live::Summary, LiveError> {
        match action.as_str() {
            "pause" => manager.get(&id)?.pause()?,
            "resume" => {
                manager.resume(&id)?;
            }
            "stop" => manager.get(&id)?.stop(flatten)?,
            "flatten" => manager.get(&id)?.flatten()?,
            "cancel-orders" => {
                manager.get(&id)?.cancel_orders()?;
            }
            other => return Err(LiveError::Invalid(format!("unknown action `{other}`"))),
        }
        Ok(manager.get(&id)?.summary())
    })
    .await
    .map_err(|e| ApiError::internal(e.to_string()))?
    .map_err(live_err)?;
    Ok(HttpResponse::Ok().json(r))
}

fn double_option<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Option<f64>>, D::Error> {
    Ok(Some(Option::<f64>::deserialize(d)?))
}

/// A missing field leaves the limit unchanged; `null` removes it.
#[derive(Deserialize)]
struct LimitsBody {
    #[serde(default, deserialize_with = "double_option")]
    max_drawdown_pct: Option<Option<f64>>,
    #[serde(default, deserialize_with = "double_option")]
    max_position_value: Option<Option<f64>>,
}

async fn live_limits(state: web::Data<AppState>, id: web::Path<String>, body: web::Json<LimitsBody>) -> ApiResult {
    let dep = state.live.get(&id).map_err(live_err)?;
    dep.set_limits(body.max_drawdown_pct, body.max_position_value).map_err(live_err)?;
    Ok(HttpResponse::Ok().json(dep.summary()))
}

async fn live_delete(state: web::Data<AppState>, id: web::Path<String>) -> ApiResult {
    state.live.delete(&id).map_err(live_err)?;
    Ok(HttpResponse::Ok().json(json!({ "ok": true })))
}

// ---- workspace seeding ----------------------------------------------------

/// Creates the workspace and fills it with the examples when it is new or empty.
pub fn prepare_workspace(dir: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let has_strategies = std::fs::read_dir(dir)?.flatten().any(|e| e.file_name().to_string_lossy().ends_with(".sm"));
    if !has_strategies && std::fs::read_dir(dir)?.flatten().all(|e| e.file_name() == "brokers" || e.file_name() == ".live") {
        for (name, content) in [
            ("threshold.sm", include_str!("../../../examples/threshold.sm")),
            ("threshold.test.json", include_str!("../../../examples/threshold.test.json")),
            ("momentum.sm", include_str!("../../../examples/momentum.sm")),
            ("momentum.test.json", include_str!("../../../examples/momentum.test.json")),
            ("bracket.sm", include_str!("../../../examples/bracket.sm")),
            ("bracket.test.json", include_str!("../../../examples/bracket.test.json")),
        ] {
            std::fs::write(dir.join(name), content)?;
        }
    }
    // Brokerage connection descriptions (read-only for the UI; edit them as files).
    let brokers = dir.join("brokers");
    if !brokers.exists() {
        std::fs::create_dir_all(&brokers)?;
        std::fs::write(brokers.join("alpaca-paper.json"), include_str!("../../../brokers/alpaca-paper.json"))?;
        std::fs::write(brokers.join("tradier-sandbox.json"), include_str!("../../../brokers/tradier-sandbox.json"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::{http::StatusCode, test as atest, App};
    use ezt_agent::llm::Request;
    use std::sync::Mutex;

    /// Replays canned model replies.
    struct Script(Mutex<Vec<Value>>);
    impl Llm for Script {
        fn complete(&self, _req: &Request) -> Result<Value, String> {
            self.0.lock().unwrap().pop().ok_or_else(|| "script exhausted".to_owned())
        }
    }

    fn state(dir: &std::path::Path, llm: Option<SharedLlm>) -> web::Data<AppState> {
        prepare_workspace(dir).unwrap();
        // The compiler binary is not needed by these tests (compile is exercised in the CLI tests).
        let checker = Arc::new(Checker::new(PathBuf::from("/nonexistent/compile"), PathBuf::from("/nonexistent/libs")));
        web::Data::new(AppState {
            dir: dir.to_path_buf(),
            checker,
            llm,
            llm_note: "no key".into(),
            model: "test-model".into(),
            stdlib: String::new(),
            allowed_hosts: vec!["studio.example".into()],
            live: live::Manager::open(dir).unwrap(),
            cluster: crate::cluster::Cluster::new(None),
        })
    }

    macro_rules! app {
        ($state:expr) => {
            atest::init_service(App::new().configure(|c| configure(c, $state.clone()))).await
        };
    }

    fn get(uri: &str) -> atest::TestRequest {
        atest::TestRequest::get().uri(uri).insert_header((header::HOST, "localhost:8787"))
    }

    fn post(uri: &str, body: Value) -> atest::TestRequest {
        atest::TestRequest::post().uri(uri).insert_header((header::HOST, "127.0.0.1:8787")).set_json(body)
    }

    #[test]
    fn host_allow_list() {
        let allowed = vec!["studio.example".to_string()];
        for ok in ["localhost", "localhost:8787", "127.0.0.1:1", "[::1]:8787", "LOCALHOST", "studio.example:443"] {
            assert!(host_allowed(Some(ok), &allowed), "{ok}");
        }
        for bad in ["evil.com", "evil.com:8787", "127.0.0.1.evil.com", "localhost.evil.com", ""] {
            assert!(!host_allowed(Some(bad), &allowed), "{bad}");
        }
        assert!(!host_allowed(None, &allowed));
    }

    #[test]
    fn file_names_are_restricted() {
        for ok in ["a.sm", "my-strategy_2.sm", "x.test.json"] {
            assert!(valid_name(ok), "{ok}");
        }
        for bad in ["", ".hidden.sm", "../x.sm", "a/b.sm", "a\\b.sm", "x.txt", "x.sm.exe", "a..sm", &"a".repeat(70)] {
            assert!(!valid_name(bad), "{bad}");
        }
    }

    #[actix_web::test]
    async fn serves_the_ui_and_rejects_foreign_hosts() {
        let dir = tempfile::tempdir().unwrap();
        let st = state(dir.path(), None);
        let app = app!(st);
        let r = atest::call_service(&app, get("/").to_request()).await;
        assert_eq!(r.status(), StatusCode::OK);
        let r = atest::call_service(&app, atest::TestRequest::get().uri("/").insert_header((header::HOST, "evil.com")).to_request()).await;
        assert_eq!(r.status(), StatusCode::NOT_FOUND);
        let r = atest::call_service(&app, atest::TestRequest::get().uri("/api/files").insert_header((header::HOST, "studio.example")).to_request()).await;
        assert_eq!(r.status(), StatusCode::OK);
    }

    #[actix_web::test]
    async fn files_round_trip_and_reject_traversal() {
        let dir = tempfile::tempdir().unwrap();
        let st = state(dir.path(), None);
        let app = app!(st);
        let body: Value = atest::call_and_read_body_json(&app, get("/api/files").to_request()).await;
        let names: Vec<_> = body["files"].as_array().unwrap().iter().map(|f| f["name"].as_str().unwrap().to_owned()).collect();
        assert_eq!(names, ["bracket.sm", "bracket.test.json", "momentum.sm", "momentum.test.json", "threshold.sm", "threshold.test.json"]);

        let put = atest::TestRequest::put().uri("/api/files/new.sm").insert_header((header::HOST, "localhost")).set_json(json!({"content": "machine M {}"})).to_request();
        assert_eq!(atest::call_service(&app, put).await.status(), StatusCode::OK);
        let got: Value = atest::call_and_read_body_json(&app, get("/api/files/new.sm").to_request()).await;
        assert_eq!(got["content"], "machine M {}");
        assert_eq!(std::fs::read_to_string(dir.path().join("new.sm")).unwrap(), "machine M {}");

        for bad in ["/api/files/..%2Fsecret.sm", "/api/files/notes.txt", "/api/files/.env.sm"] {
            let r = atest::call_service(&app, get(bad).to_request()).await;
            assert!(r.status() == StatusCode::BAD_REQUEST || r.status() == StatusCode::NOT_FOUND, "{bad}: {}", r.status());
        }
        let put = atest::TestRequest::put().uri("/api/files/..%2Fescape.sm").insert_header((header::HOST, "localhost")).set_json(json!({"content": "x"})).to_request();
        assert_ne!(atest::call_service(&app, put).await.status(), StatusCode::OK);
        assert!(!dir.path().parent().unwrap().join("escape.sm").exists());
        assert_eq!(atest::call_service(&app, get("/api/files/missing.sm").to_request()).await.status(), StatusCode::NOT_FOUND);

        // Non-JSON mutations are refused.
        let put = atest::TestRequest::put().uri("/api/files/x.sm").insert_header((header::HOST, "localhost")).insert_header((header::CONTENT_TYPE, "text/plain")).set_payload("{\"content\":\"x\"}").to_request();
        assert_eq!(atest::call_service(&app, put).await.status(), StatusCode::BAD_REQUEST);
    }

    #[actix_web::test]
    async fn analyze_returns_diagram_params_and_positioned_diagnostics() {
        let dir = tempfile::tempdir().unwrap();
        let st = state(dir.path(), None);
        let app = app!(st);
        let src = include_str!("../../../examples/threshold.sm");
        let r: Value = atest::call_and_read_body_json(&app, post("/api/analyze", json!({"source": src})).to_request()).await;
        assert_eq!(r["ok"], true);
        assert_eq!(r["machines"], json!(["Threshold"]));
        assert!(r["diagram"]["mermaid"].as_str().unwrap().starts_with("flowchart TB"));
        let names: Vec<_> = r["params"].as_array().unwrap().iter().map(|p| p["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["equities", "account", "invest", "floor", "ceiling"]);
        assert_eq!(r["params"][0]["hint"], "all symbols in the data");
        assert_eq!(r["params"][3]["default"], "10");
        assert_eq!(r["params"][3]["required"], false);

        let r: Value = atest::call_and_read_body_json(&app, post("/api/analyze", json!({"source": "machine M {\n init { launch (p, A) }\n}"})).to_request()).await;
        assert_eq!(r["ok"], false);
        assert_eq!(r["diagnostics"][0]["line"], 2);
        assert_eq!(r["diagnostics"][0]["severity"], "error");

        // A machine name left over from another file does not break the analysis.
        let r: Value = atest::call_and_read_body_json(&app, post("/api/analyze", json!({"source": src, "machine": "FromAnotherFile"})).to_request()).await;
        assert!(r["diagram"]["mermaid"].as_str().unwrap().starts_with("flowchart TB"));
        assert_eq!(r["params"].as_array().unwrap().len(), 5);

        let bad = "machine M {\n init { launch (p, A); }\n process p {\n  state A: goto Z;\n }\n}";
        let r: Value = atest::call_and_read_body_json(&app, post("/api/analyze", json!({"source": bad})).to_request()).await;
        assert_eq!(r["diagnostics"][0]["line"], 4);
    }

    #[actix_web::test]
    async fn backtest_with_generated_and_csv_data() {
        let dir = tempfile::tempdir().unwrap();
        let st = state(dir.path(), None);
        let app = app!(st);
        let src = include_str!("../../../examples/threshold.sm");
        let req = json!({
            "source": src,
            "params": {"floor": 95, "ceiling": 105},
            "data": {"kind": "generate", "symbol": "ABC", "generator": {"kind": "sine", "bars": 120, "period": 40, "amplitude": 0.2, "vol": 0.0}},
        });
        let r: Value = atest::call_and_read_body_json(&app, post("/api/backtest", req).to_request()).await;
        assert_eq!(r["ok"], true, "{r}");
        let b = &r["backtest"];
        assert_eq!(b["symbol"], "ABC");
        assert_eq!(b["equity"].as_array().unwrap().len(), 120);
        assert!(b["trades"].as_array().unwrap().len() >= 4, "{}", b["trades"]);
        assert_eq!(r["golden"]["symbol"], "ABC");
        assert_eq!(r["golden"]["prices"].as_array().unwrap().len(), 120);

        // The golden test it returns passes when fed back.
        let golden = serde_json::to_string(&json!({"tests": [r["golden"].clone()]})).unwrap();
        let t: Value = atest::call_and_read_body_json(&app, post("/api/test", json!({"source": src, "tests": golden})).to_request()).await;
        assert_eq!((t["ok"].clone(), t["passed"].clone(), t["failed"].clone()), (json!(true), json!(1), json!(0)), "{t}");

        let csv = "date,close\n2024-01-01,12\n2024-01-02,9\n2024-01-03,9\n2024-01-04,16\n2024-01-05,16\n";
        let req = json!({"source": src, "data": {"kind": "csv", "symbol": "XYZ", "csv": csv}});
        let r: Value = atest::call_and_read_body_json(&app, post("/api/backtest", req).to_request()).await;
        assert_eq!(r["ok"], true, "{r}");
        assert_eq!(r["backtest"]["bars"][1]["label"], "2024-01-02");
        assert_eq!(r["backtest"]["trades"][0]["side"], "buy");
    }

    #[actix_web::test]
    async fn backtest_reports_domain_errors_with_ok_false() {
        let dir = tempfile::tempdir().unwrap();
        let st = state(dir.path(), None);
        let app = app!(st);
        let gen = json!({"kind": "generate", "generator": {"bars": 10}});
        let r: Value = atest::call_and_read_body_json(&app, post("/api/backtest", json!({"source": "machine M {", "data": gen.clone()})).to_request()).await;
        assert_eq!((r["ok"].clone(), r["error"]["line"].clone()), (json!(false), json!(1)));
        let r: Value = atest::call_and_read_body_json(&app, post("/api/backtest", json!({"source": include_str!("../../../examples/threshold.sm"), "data": {"kind": "csv", "csv": "abc"}})).to_request()).await;
        assert_eq!(r["ok"], false);
        let r: Value = atest::call_and_read_body_json(&app, post("/api/backtest", json!({"source": "machine M { init { launch (p, A); } process p { state A: goto A { record x in y; } } }", "data": gen})).to_request()).await;
        assert!(r["error"]["message"].as_str().unwrap().contains("not simulated"), "{r}");
        assert!(r["error"]["line"].is_number());
        assert_eq!(atest::call_service(&app, post("/api/backtest", json!({"nonsense": 1})).to_request()).await.status(), StatusCode::BAD_REQUEST);
    }

    #[actix_web::test]
    async fn tests_endpoint_runs_the_shipped_examples() {
        let dir = tempfile::tempdir().unwrap();
        let st = state(dir.path(), None);
        let app = app!(st);
        let req = json!({"source": include_str!("../../../examples/momentum.sm"), "tests": include_str!("../../../examples/momentum.test.json")});
        let t: Value = atest::call_and_read_body_json(&app, post("/api/test", req).to_request()).await;
        assert_eq!((t["passed"].clone(), t["failed"].clone()), (json!(3), json!(0)), "{t}");
        let t: Value = atest::call_and_read_body_json(&app, post("/api/test", json!({"source": "machine M {}", "tests": "nope"})).to_request()).await;
        assert_eq!(t["ok"], false);
    }

    #[actix_web::test]
    async fn ai_is_disabled_without_a_model_and_runs_the_agent_with_one() {
        let dir = tempfile::tempdir().unwrap();
        let app = app!(state(dir.path(), None));
        let s: Value = atest::call_and_read_body_json(&app, get("/api/ai/status").to_request()).await;
        assert_eq!(s["enabled"], false);
        let r: Value = atest::call_and_read_body_json(&app, post("/api/ai/explain", json!({"source": "machine M {}"})).to_request()).await;
        assert_eq!((r["ok"].clone(), r["error"].clone()), (json!(false), json!("no key")));

        // With a (scripted) model: explain returns text and no tools are involved.
        let llm: SharedLlm = Arc::new(Script(Mutex::new(vec![json!({"stop_reason": "end_turn", "content": [{"type": "text", "text": "It does nothing."}], "usage": {"input_tokens": 3, "output_tokens": 4}})])));
        let dir2 = tempfile::tempdir().unwrap(); // a second app needs its own workspace (deployments are locked per folder)
        let app = app!(state(dir2.path(), Some(llm)));
        let s: Value = atest::call_and_read_body_json(&app, get("/api/ai/status").to_request()).await;
        assert_eq!((s["enabled"].clone(), s["model"].clone()), (json!(true), json!("test-model")));
        let r: Value = atest::call_and_read_body_json(&app, post("/api/ai/explain", json!({"source": "machine M {}"})).to_request()).await;
        assert_eq!(r["summary"], "It does nothing.");
        assert_eq!(r["usage"]["output_tokens"], 4);

        let r = atest::call_service(&app, post("/api/ai/generate", json!({"prompt": "  "})).to_request()).await;
        assert_eq!(r.status(), StatusCode::BAD_REQUEST);
        let r = atest::call_service(&app, post("/api/ai/frobnicate", json!({})).to_request()).await;
        assert_eq!(r.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn origin_allow_list() {
        let allowed = vec!["studio.example".to_string()];
        for ok in [None, Some("http://localhost:8787"), Some("http://127.0.0.1:8787"), Some("https://studio.example")] {
            assert!(origin_allowed(ok, &allowed), "{ok:?}");
        }
        for bad in ["http://evil.com", "https://evil.com:8787", "null", "http://localhost.evil.com", "file://x", ""] {
            assert!(!origin_allowed(Some(bad), &allowed), "{bad}");
        }
    }

    #[actix_web::test]
    async fn brokerage_connections_over_http() {
        use ezt_broker::fake::{FakeServer, Flavor};
        let fake = FakeServer::start(Flavor::Alpaca, "key-123", "", 50_000.0);
        fake.price("ACME", 100.0);
        let (dir, brokers) = crate::brokers::tests::workspace_with("alpaca-paper.json", &fake.url());
        prepare_workspace(dir.path()).unwrap();
        let checker = Arc::new(Checker::new(PathBuf::from("/nonexistent/compile"), PathBuf::from("/nonexistent/libs")));
        let st = web::Data::new(AppState {
            dir: dir.path().to_path_buf(),
            checker,
            llm: None,
            llm_note: String::new(),
            model: "m".into(),
            stdlib: String::new(),
            allowed_hosts: vec![],
            live: live::Manager::open_with(dir.path(), Arc::new(brokers)).unwrap(),
            cluster: crate::cluster::Cluster::new(None),
        });
        let app = app!(st);
        let l: Value = atest::call_and_read_body_json(&app, get("/api/brokers").to_request()).await;
        assert_eq!(l["brokers"][0]["name"], "alpaca-paper", "{l}");
        assert_eq!((l["brokers"][0]["ready"].clone(), l["kill_switch"].clone(), l["allow_live"].clone()), (json!(true), json!(false), json!(false)));
        let t: Value = atest::call_and_read_body_json(&app, post("/api/brokers/alpaca-paper/test", json!({"symbol": "ACME"})).to_request()).await;
        assert_eq!(t["ok"], true, "{t}");
        assert_eq!(t["steps"].as_array().unwrap().len(), 5);
        let bad = atest::call_service(&app, post("/api/brokers/..%2Fx/test", json!({})).to_request()).await;
        assert!(bad.status().is_client_error());
        let t: Value = atest::call_and_read_body_json(&app, post("/api/brokers/missing/test", json!({})).to_request()).await;
        assert_eq!(t["ok"], false);

        let mut body = deploy_body(json!({"kind": "broker", "broker": "alpaca-paper", "symbols": ["ACME"], "tick_ms": 250}));
        body["execution"] = json!({"kind": "broker", "broker": "alpaca-paper"});
        let s: Value = atest::call_and_read_body_json(&app, post("/api/live", body).to_request()).await;
        assert_eq!((s["status"].clone(), s["execution"].clone()), (json!("running"), json!("alpaca-paper")), "{s}");
        let id = s["id"].as_str().unwrap().to_owned();
        fake.price("ACME", 90.0);
        let mut detail = Value::Null;
        for _ in 0..200 {
            detail = atest::call_and_read_body_json(&app, get(&format!("/api/live/{id}")).to_request()).await;
            if !detail["positions"].as_array().unwrap().is_empty() { break; }
            actix_web::rt::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert_eq!(detail["broker"]["name"], "alpaca-paper");
        assert_eq!(detail["orders"][0]["state"], "filled", "{detail}");

        let e: Value = atest::call_and_read_body_json(&app, post("/api/live/emergency-stop", json!({})).to_request()).await;
        assert_eq!(e["paused"], json!([id]));
        let l: Value = atest::call_and_read_body_json(&app, get("/api/brokers").to_request()).await;
        assert_eq!(l["kill_switch"], true);
        assert_eq!(atest::call_service(&app, post(&format!("/api/live/{id}/resume"), json!({})).to_request()).await.status(), StatusCode::CONFLICT);
        let k: Value = atest::call_and_read_body_json(&app, post("/api/brokers/kill", json!({"on": false})).to_request()).await;
        assert_eq!(k["kill_switch"], false);
        let r: Value = atest::call_and_read_body_json(&app, post(&format!("/api/live/{id}/resume"), json!({})).to_request()).await;
        assert_eq!(r["status"], "running");
        let r: Value = atest::call_and_read_body_json(&app, post(&format!("/api/live/{id}/cancel-orders"), json!({})).to_request()).await;
        assert_eq!(r["id"], id);
        st.live.shutdown();
    }

    fn deploy_body(feed: Value) -> Value {
        json!({
            "name": "api test",
            "source": include_str!("../../../examples/threshold.sm"),
            "strategy_file": "threshold.sm",
            "params": {"floor": 95, "ceiling": 105},
            "feed": feed,
        })
    }

    #[actix_web::test]
    async fn live_lifecycle_over_http() {
        let dir = tempfile::tempdir().unwrap();
        let st = state(dir.path(), None);
        let app = app!(st);
        let l: Value = atest::call_and_read_body_json(&app, get("/api/live").to_request()).await;
        assert_eq!(l["deployments"], json!([]));
        assert_eq!(l["max_running"], live::MAX_RUNNING);

        let feed = json!({"kind": "simulated", "generator": {"kind": "sine", "seed": 3, "vol": 0.004, "period": 40, "amplitude": 0.2}, "tick_ms": 20});
        let s: Value = atest::call_and_read_body_json(&app, post("/api/live", deploy_body(feed)).to_request()).await;
        assert_eq!(s["status"], "running", "{s}");
        let id = s["id"].as_str().unwrap().to_owned();

        // wait for some bars, then read the detail with cursors
        let mut n = 0;
        for _ in 0..300 {
            let d: Value = atest::call_and_read_body_json(&app, get(&format!("/api/live/{id}")).to_request()).await;
            n = d["summary"]["bars"].as_u64().unwrap();
            if n >= 5 { break; }
            actix_web::rt::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(n >= 5);
        let d: Value = atest::call_and_read_body_json(&app, get(&format!("/api/live/{id}?bars_from={n}&after_seq=1000000")).to_request()).await;
        assert_eq!(d["series"]["from"], n);
        assert_eq!(d["events"], json!([]));
        assert!(d["processes"][0]["states"].is_array());
        let src: Value = atest::call_and_read_body_json(&app, get(&format!("/api/live/{id}/source")).to_request()).await;
        assert!(src["source"].as_str().unwrap().contains("machine Threshold"));
        assert_eq!(src["strategy_file"], "threshold.sm");

        let r: Value = atest::call_and_read_body_json(&app, post(&format!("/api/live/{id}/pause"), json!({})).to_request()).await;
        assert_eq!(r["status"], "paused");
        let r: Value = atest::call_and_read_body_json(&app, post(&format!("/api/live/{id}/resume"), json!({})).to_request()).await;
        assert_eq!(r["status"], "running");
        let lim = atest::TestRequest::put().uri(&format!("/api/live/{id}/limits")).insert_header((header::HOST, "localhost")).set_json(json!({"max_drawdown_pct": 15.0, "max_position_value": null})).to_request();
        let r: Value = atest::call_and_read_body_json(&app, lim).await;
        assert_eq!(r["id"], id);
        let d: Value = atest::call_and_read_body_json(&app, get(&format!("/api/live/{id}")).to_request()).await;
        assert_eq!(d["limits"]["max_drawdown_pct"], 15.0);

        // deleting a running deployment is a conflict; stopping then deleting works
        let del = atest::TestRequest::delete().uri(&format!("/api/live/{id}")).insert_header((header::HOST, "localhost")).to_request();
        assert_eq!(atest::call_service(&app, del).await.status(), StatusCode::CONFLICT);
        let r: Value = atest::call_and_read_body_json(&app, post(&format!("/api/live/{id}/stop"), json!({"flatten": true})).to_request()).await;
        assert_eq!(r["status"], "stopped");
        let del = atest::TestRequest::delete().uri(&format!("/api/live/{id}")).insert_header((header::HOST, "localhost")).to_request();
        assert_eq!(atest::call_service(&app, del).await.status(), StatusCode::OK);
        assert_eq!(atest::call_service(&app, get(&format!("/api/live/{id}")).to_request()).await.status(), StatusCode::NOT_FOUND);
        st.live.shutdown();
    }

    #[actix_web::test]
    async fn live_rejects_bad_requests_and_cross_site_writes() {
        let dir = tempfile::tempdir().unwrap();
        let st = state(dir.path(), None);
        let app = app!(st);
        // invalid strategy -> 400 with the message
        let mut body = deploy_body(json!({"kind": "simulated"}));
        body["source"] = json!("machine M {\n init { launch(p, A) }\n}");
        let r = atest::call_service(&app, post("/api/live", body).to_request()).await;
        assert_eq!(r.status(), StatusCode::BAD_REQUEST);
        let msg: Value = atest::read_body_json(r).await;
        assert!(msg["error"].as_str().unwrap().contains("syntax error on line 2"), "{msg}");
        // Twelve Data without a key is a clear 400, not a crash
        std::env::remove_var("TWELVEDATA_API_KEY");
        let r = atest::call_service(&app, post("/api/live", deploy_body(json!({"kind": "twelve_data", "symbols": ["AAPL"]}))).to_request()).await;
        assert_eq!(r.status(), StatusCode::BAD_REQUEST);
        // unknown deployment / action
        assert_eq!(atest::call_service(&app, post("/api/live/nope/pause", json!({})).to_request()).await.status(), StatusCode::NOT_FOUND);

        let s: Value = atest::call_and_read_body_json(&app, post("/api/live", deploy_body(json!({"kind": "simulated", "tick_ms": 3600000}))).to_request()).await;
        let id = s["id"].as_str().unwrap();
        assert_eq!(atest::call_service(&app, post(&format!("/api/live/{id}/explode"), json!({})).to_request()).await.status(), StatusCode::NOT_FOUND);

        // CSRF: a "simple" cross-site POST (text/plain, no preflight) is refused...
        let plain = atest::TestRequest::post().uri(&format!("/api/live/{id}/stop")).insert_header((header::HOST, "127.0.0.1:8787")).insert_header((header::CONTENT_TYPE, "text/plain")).set_payload("{}").to_request();
        assert_eq!(atest::call_service(&app, plain).await.status(), StatusCode::BAD_REQUEST);
        // ...and so is a JSON one that declares a foreign Origin.
        let foreign = atest::TestRequest::post().uri(&format!("/api/live/{id}/stop")).insert_header((header::HOST, "127.0.0.1:8787")).insert_header((header::ORIGIN, "http://evil.com")).set_json(json!({})).to_request();
        assert_eq!(atest::call_service(&app, foreign).await.status(), StatusCode::NOT_FOUND);
        let d: Value = atest::call_and_read_body_json(&app, get(&format!("/api/live/{id}")).to_request()).await;
        assert_eq!(d["summary"]["status"], "running", "neither request reached the deployment");
        // a same-origin one works
        let ok = atest::TestRequest::post().uri(&format!("/api/live/{id}/stop")).insert_header((header::HOST, "127.0.0.1:8787")).insert_header((header::ORIGIN, "http://127.0.0.1:8787")).set_json(json!({})).to_request();
        assert_eq!(atest::call_service(&app, ok).await.status(), StatusCode::OK);
        st.live.shutdown();
    }

    #[test]
    fn compiler_lines_become_positioned_diagnostics() {
        let d = compiler_diagnostics(&["strategy.sm:(3,7): syntax error: expected x".into(), "lint: `goto B` ...".into(), "weird output".into()]);
        assert_eq!(d.len(), 2);
        assert_eq!((d[0].line, d[0].col, d[0].message.as_str()), (3, 7, "syntax error: expected x"));
        assert_eq!((d[1].line, d[1].message.as_str()), (1, "weird output"));
    }

    #[test]
    fn seeds_only_an_empty_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        prepare_workspace(&ws).unwrap();
        assert_eq!(std::fs::read_dir(&ws).unwrap().count(), 7, "six example files and the brokers folder");
        assert!(ws.join("brokers/alpaca-paper.json").is_file());
        std::fs::remove_file(ws.join("momentum.sm")).unwrap();
        prepare_workspace(&ws).unwrap();
        assert!(!ws.join("momentum.sm").exists(), "existing workspaces are left alone");
    }
}
