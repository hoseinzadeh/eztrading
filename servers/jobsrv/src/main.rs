use clap::Parser;
use com::{net::Server, status::Status};

mod jobsrv;

/// The Job Server (JobSrv) for managing schedulers and state machines
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// The address of DataSrv (e.g., "10.1.1.5:2020")
    #[arg(short('s'), long)]
    datasrv: String,

    /// The IP address of Database Server
    #[arg(long, default_value_t = String::from("127.0.0.1:3306"))]
    db: String,

    /// The username of Database Server
    #[arg(long, default_value_t = String::from("root"))]
    db_user: String,

    /// The password of Database Server
    #[arg(long, default_value_t = String::from("12345678"))]
    db_password: String,

    /// Number of schedulers
    #[arg(short, long, default_value_t = 1)]
    capacity: usize,

    /// The workspace directory
    #[arg(short, long, default_value_t = String::from("/tmp/eztrading"))]
    workspace: String,

    /// Path to scheduler.dylib
    #[arg(long, default_value_t = String::from("package/common/scheduler.dylib"))]
    sch_lib: String,

    /// The IP address of the SrvMan (e.g., "127.0.0.1:2030")
    #[arg(long)]
    srvman: Option<String>,

    /// An id assigned by the SrvMan (required when using `srvman`)
    #[arg(long)]
    id: Option<usize>,
}

fn main() -> Result<(), Status> {
    let args = Args::parse();
    let mut jobsrv = jobsrv::JobSrv::new(
        args.sch_lib,
        &args.datasrv,
        args.db,
        args.db_user,
        args.db_password,
        args.workspace,
        args.srvman.map(|a| (a, args.id.expect("id is required"))),
    )?;
    jobsrv.setup(args.capacity);
    // jobsrv.load_plugin("tests/example.dylib\0".to_owned()).unwrap();
    jobsrv.start();

    Ok(())
}
