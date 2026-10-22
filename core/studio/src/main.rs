//! `ezt-studio`: the Strategy Studio server. Run it, open the printed URL.

use actix_web::{web, App, HttpServer};
use clap::Parser;
use ezt_agent::compiler::Checker;
use ezt_agent::llm::Anthropic;
use std::path::PathBuf;
use std::sync::Arc;
use studio::server::{self, AppState, SharedLlm};

#[derive(Parser)]
#[command(name = "ezt-studio", version, about = "A browser editor for SML strategies: live diagnostics, state diagrams, backtests, tests and AI assist", long_about = None)]
struct Args {
    /// Workspace directory holding your .sm strategies and .test.json files (created and filled with examples if new)
    #[arg(long, default_value = "strategies")]
    dir: PathBuf,

    /// Address to listen on. Keep it on loopback unless you know what you are doing.
    #[arg(long, default_value = "127.0.0.1")]
    host: String,

    #[arg(long, default_value_t = 8787)]
    port: u16,

    /// Extra Host header name to accept (needed behind a reverse proxy). May be repeated.
    #[arg(long = "allow-host")]
    allow_host: Vec<String>,

    /// Path to the SML compiler (`compile`). Defaults to the one next to this binary, then PATH.
    #[arg(long, env = "EZT_COMPILER")]
    compiler: Option<PathBuf>,

    /// Directory with the SML standard library
    #[arg(long, env = "EZT_STDLIB", default_value = "package/libs")]
    stdlib: PathBuf,

    /// Claude model used by the AI features
    #[arg(long, env = "EZT_AGENT_MODEL", default_value = "claude-opus-5-5")]
    model: String,

    /// Reasoning effort for the AI features: low, medium, high, xhigh or max
    #[arg(long, env = "EZT_AGENT_EFFORT")]
    effort: Option<String>,

    /// Turn the AI features off even if ANTHROPIC_API_KEY is set
    #[arg(long)]
    no_ai: bool,

    /// Allow deployments to trade through brokerage connections that use real money (environment "live").
    /// Each such deployment additionally needs typed confirmation and order/position value limits.
    #[arg(long)]
    allow_live: bool,
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    let args = Args::parse();
    server::prepare_workspace(&args.dir)?;

    let stdlib_text = std::fs::read_to_string(args.stdlib.join("std.sm")).unwrap_or_else(|e| {
        eprintln!("warning: cannot read {}: {e} (run from the repository root or pass --stdlib); the AI features need it", args.stdlib.join("std.sm").display());
        String::new()
    });
    let checker = Arc::new(Checker::new(Checker::locate_compiler(args.compiler.as_deref()), args.stdlib.clone()));

    let (llm, llm_note): (Option<SharedLlm>, String) = if args.no_ai {
        (None, "AI features are turned off (--no-ai).".into())
    } else {
        // The client is built on a blocking thread: reqwest's blocking client must not be
        // created (or dropped) inside the async runtime.
        let (model, effort) = (args.model.clone(), args.effort.clone());
        match web::block(move || Anthropic::from_env(&model, effort)).await {
            Ok(Ok(client)) => (Some(Arc::new(client)), String::new()),
            Ok(Err(e)) => (None, e),
            Err(e) => (None, format!("could not start the AI client: {e}")),
        }
    };

    let brokers = Arc::new(studio::brokers::Brokers::from_process_env(&args.dir, args.allow_live));
    let connections = brokers.list();
    if !connections.is_empty() {
        println!("Brokerage connections: {}", connections.iter().map(|b| format!("{}{}", b.name, if b.ready { "" } else { " (not ready)" })).collect::<Vec<_>>().join(", "));
    }
    if args.allow_live {
        eprintln!("warning: --allow-live is on: deployments may trade real money through connections whose environment is \"live\".");
    }
    // Restoring may talk to a brokerage (blocking HTTP), so it must not run on the async worker.
    let dir = args.dir.clone();
    let live_manager = web::block(move || studio::live::Manager::open_with(&dir, brokers))
        .await
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    let restored = live_manager.list();
    if !restored.is_empty() {
        let running = restored.iter().filter(|d| d.status == studio::live::Status::Running).count();
        println!("Restored {} deployment(s); {running} running", restored.len());
    }

    let state = web::Data::new(AppState {
        dir: args.dir.clone(),
        checker,
        llm,
        llm_note,
        model: args.model.clone(),
        stdlib: stdlib_text,
        allowed_hosts: args.allow_host.clone(),
        live: live_manager.clone(),
    });

    let url = format!("http://{}:{}/", if args.host == "0.0.0.0" { "localhost" } else { &args.host }, args.port);
    println!("Strategy Studio is running at {url}");
    println!("Workspace: {}", args.dir.display());
    if args.host != "127.0.0.1" && args.host != "localhost" && args.host != "::1" {
        eprintln!("warning: listening on {}; Studio has no login and can run AI requests with your API key. Keep it on loopback or put it behind an authenticating proxy.", args.host);
    }
    let result = HttpServer::new(move || App::new().configure(|c| server::configure(c, state.clone())))
        .bind((args.host.as_str(), args.port))?
        .run()
        .await;
    // Stop the deployment threads; their state is already on disk and they resume on the next start.
    live_manager.shutdown();
    result
}
