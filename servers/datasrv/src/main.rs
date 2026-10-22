mod datasrv;
mod providers;
// mod ws;
use clap::Parser;
use com::{net::Server, status::Status};
use providers::twelvedata::TwelveData;

/// The Data Server (DataSrv) for managing data
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// The IP address of DataSrv (e.g., "127.0.0.1:2020")
    #[arg(long, default_value_t = String::from("127.0.0.1:2020"))]
    ip: String,

    /// The IP address of Database Server
    #[arg(long, default_value_t = String::from("127.0.0.1:3306"))]
    db: String,

    /// The username of Database Server
    #[arg(long, default_value_t = String::from("root"))]
    db_user: String,

    /// The password of Database Server
    #[arg(long, env = "EZTRADING_DB_PASSWORD", default_value_t = String::from("123456"))]
    db_password: String,

    /// The workspace directory
    #[arg(short, long, default_value_t = String::from("/tmp/eztrading"))]
    workspace: String,

    /// The Twelve Data API key (or set TWELVEDATA_API_KEY)
    #[arg(long, short('k'), env = "TWELVEDATA_API_KEY", hide_env_values = true)]
    apikey: String,

    /// The IP address of SrvMan (e.g., "127.0.0.1:2030")
    #[arg(long)]
    srvman: Option<String>,

    /// An id assigned by the SrvMan (required when using `srvman`)
    #[arg(long)]
    id: Option<usize>,
}

fn main() -> Result<(), Status> {
    let args = Args::parse();
    let apikey = args.apikey;
    let socket = com::listen!(&args.ip).expect("Listener failed to bind");
    let mut datasrv = datasrv::DataSrv::<TwelveData>::new(
        socket,
        args.srvman.map(|a| (a, args.id.expect("id is required"))),
        &args.workspace,
        &apikey,
        "secret/cert.pem", "secret/key.rsa", ([0,0,0,0], 9231)
    )?;

    datasrv.start();

    Ok(())
}