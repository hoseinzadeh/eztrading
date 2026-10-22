use codesrv::CodeSrv;
use com::net::Server;
mod codesrv;
use clap::Parser;

/// The Code Server (CodeSrv) for building plugins
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// The IP address of CodeSrv
    #[arg(long, default_value_t = String::from("127.0.0.1:2022"))]
    ip: String,

    /// The IP address of Database Server
    #[arg(long, default_value_t = String::from("127.0.0.1:3306"))]
    db: String,

    /// The username of Database Server
    #[arg(long, default_value_t = String::from("root"))]
    db_user: String,

    /// The password of Database Server
    #[arg(long, default_value_t = String::from("12345678"))]
    db_password: String,

    /// Path to the workspace directory
    #[arg(long, short('w'))]
    workspace: Option<String>,

    /// Path to the code template directory
    #[arg(long, short('t'))]
    template: Option<String>,

    /// Path to the build script
    #[arg(long, short('b'))]
    build_script: Option<String>,

    /// The IP address of SrvMan (e.g., "127.0.0.1:2030")
    #[arg(long)]
    srvman: Option<String>,

    /// An id assigned by the SrvMan (required when using `srvman`)
    #[arg(long)]
    id: Option<usize>,
}

fn main() {
    let args = Args::parse();
    let server = com::listen!(&args.ip).expect("Listener failed to bind");
    let ws = args.workspace.unwrap_or("/tmp/eztrading/codesrv".into());
    let tmp = args.template.unwrap_or("package".into());
    let build = args.build_script.unwrap_or("./pack.sh".into());
    if let Err(e) = std::fs::create_dir_all(&ws) {
        eprintln!("{e}");
        return;
    }
    // TODO: check tmp
    let mut codesrv = CodeSrv::new(
        server,
        ws,
        tmp,
        build,
        args.db,
        args.db_user,
        args.db_password,
        args.srvman.map(|a| (a, args.id.expect("id is required"))),
    )
    .expect("CodeSrv failed to bind");
    codesrv.start()
}
