use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use std::{fmt::Debug, collections::HashMap};
use super::*;
use crate::{data::{Machine, Ticker, Param}, status::Status, failed};

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub enum JobSrvError {
    PluginNotFound,
    MachineNotFound,
    Shutdown,
    Err(Status),
}

impl ToString for JobSrvError {
    fn to_string(&self) -> String {
        match self {
            JobSrvError::PluginNotFound => "PluginNotFound".into(),
            JobSrvError::MachineNotFound => "MachineNotFound".into(),
            JobSrvError::Shutdown => "Shutdown".into(),
            JobSrvError::Err(e) => e.to_string(),
        }
    }
}

impl From<String> for JobSrvError {
    fn from(value: String) -> Self {
        JobSrvError::Err(failed!(value))
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub enum JobSrvInterface {
    Request(JobSrvReq),
    Reply(JobSrvRep),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum JobSrvReq {
    /// Print a text message
    Log(String),

    /// Receive the latest value of symbol
    Update(Symbol, Ticker),

    /// Start receiving real-time prices of the given symbols
    Listen(Vec<Symbol>),

    /// Emit a list of signals with values
    Emit(Vec<(SignalId, Option<Vec<u8>>)>),

    /// Copy a file to JobSrv (plugin or cache)
    CopyData(FilePath, Vec<u8>),

    /// Load a plugin after sending the library file. If plugin was not in cached,
    /// download it from the the database and cache it for further use.
    /// The second parameter is to force download the file, anyways.
    LoadPlugin(PluginId, bool),

    /// Launch a machine for a user with given parameters
    Launch(UserId, Machine),

    /// Stop a machine for a user
    Stop(RunId),

    /// Return a list
    List(JobSrvListReq),

    /// Request the number of running jobs
    RunningJobs,

    /// Shutdown the server
    Shutdown,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum JobSrvRep {
    /// Error
    Error(JobSrvError),

    /// Todo
    Todo(JobSrvReq),

    /// Replay with a list of plugins
    Plugins(HashMap<PluginId, (PluginName, Vec<Machine>)>),

    /// Replay with a single plugin info
    Plugin(PluginName, Vec<Machine>),

    /// Replay with the list of all available plugins from the database
    AllPlugins(HashMap<PluginId, PluginDBEntry>),

    /// Replay with a list of machines
    Machines(HashMap<MachineName, Machine>),

    /// Replay with a list of machines
    MachinesList(Vec<Machine>),

    /// Replay with a list of parameters
    Params(Vec<Param>),

    /// Return the number of running jobs
    RunningJobs(usize),

    /// Operation was successful
    Successful,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum JobSrvListReq {
    /// Return the list of all available plug-ins from database
    AllPlugins,

    /// Return the list of all loaded plug-ins
    Plugins,

    /// Return the list of all loaded machines
    Machines,

    /// Return the list of machines in a plugin
    PluginMachines(PluginId),

    /// Return the list of a given machine's parameters
    MachineParams(MachineName),
}

impl From<Packet> for JobSrvReq {
    fn from(value: Packet) -> Self {
        value.unpack()
    }
}
impl From<Packet> for JobSrvRep {
    fn from(value: Packet) -> Self {
        value.unpack()
    }
}

impl From<JobSrvError> for JobSrvRep {
    fn from(e: JobSrvError) -> Self {
        JobSrvRep::Error(e)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PluginDBEntry {
    pub name: PluginName,
    pub owner_id: UserId,
    pub owner_name: String,
    pub owner_email: String,
    pub version: String,
    pub price: Option<f32>,
    pub date: Option<NaiveDate>,
    pub published: bool,
    pub machines: Option<Vec<Machine>>,
}