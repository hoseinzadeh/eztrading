//! This module reads system.toml and starts up all servers. Then it runs an
//! HTTP/2 server to allow clients connect to ezTrading web api.

mod srvman;
mod https;

use std::{time::Duration};

use com::{net::Server, status::Status, convert};
use srvman::SrvMan;
use std::thread;

fn main() -> Result<(), Status> {
    let mut sys = SrvMan::new()?;
    let ready = sys.ready.clone();
    while !ready.load(std::sync::atomic::Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(100))
    }
    let tx = sys.tx_channel();
    let https_addr = format!("{}:{}", sys.system.host, sys.system.http_port);
    let builder = thread::Builder::new().name("system".to_owned());
    let thread = builder.spawn(move || sys.start()).unwrap();

    convert!(https::start(https_addr.as_str()))?;
    SrvMan::send_shutdown_signal(&tx)?;

    thread.join().unwrap();
    Ok(())
}
