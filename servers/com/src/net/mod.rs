use serde::{Deserialize, Serialize};
use std::fmt::{Debug, Display};

pub mod man;
mod codesrv;
mod datasrv;
mod jobsrv;
mod srvman;
mod packet;
mod server;
mod ws;

pub use codesrv::*;
pub use datasrv::*;
pub use jobsrv::*;
pub use srvman::*;
pub use packet::*;
pub use server::*;
pub use ws::*;

pub type Symbol = String;
pub type FilePath = String;
pub type PluginName = String;
pub type MachineName = String;
pub type MachineParameters = String;
pub type SignalId = String;
pub type UserId = u32;
pub type PluginId = u32;
pub type MachineId = u32;
pub type JobId = u64;
pub type RunId = u32;
pub type SubscriptionId = u32;


#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum Handshake {
    /// The server is a JobSrv
    JobSrv(u64),

    /// The server is a DataSrv
    DataSrv(u64),

    /// The server is a CodeSrv
    CodeSrv(u64),

    /// The server is a SrvMan
    SrvMan(u64),

    /// The Command Line Interface
    CLI(u64),
}

impl From<Packet> for Handshake {
    fn from(value: Packet) -> Self {
       value.unpack()
    }
}

#[inline]
pub fn to_url_query<T: Display>(name: &str, val: &Option<T>) -> String {
    val.as_ref()
        .map(|v| format!("&{name}={v}"))
        .unwrap_or(String::new())
}

impl Handshake {
    pub fn secret(&self) -> u64 {
        match self {
            Handshake::JobSrv(s)
            | Handshake::DataSrv(s)
            | Handshake::CodeSrv(s)
            | Handshake::SrvMan(s)
            | Handshake::CLI(s) => *s,
        }
    }

    pub fn new_secret(&self, secret: u64) -> Self {
        match self {
            Handshake::JobSrv(_) => Handshake::JobSrv(secret),
            Handshake::DataSrv(_) => Handshake::DataSrv(secret),
            Handshake::CodeSrv(_) => Handshake::CodeSrv(secret),
            Handshake::SrvMan(_) => Handshake::SrvMan(secret),
            Handshake::CLI(_) => Handshake::CLI(secret),
        }
    }
}
