use crate::{
    data::{Machine, Param},
    failed,
    status::Status,
};

use super::*;
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, fmt::Debug};

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub enum SrvManError {
    PluginNotFound,
    MachineNotFound,
    ConnectionLost,
    Shutdown,
    Err(Status),
}

impl ToString for SrvManError {
    fn to_string(&self) -> String {
        match self {
            SrvManError::PluginNotFound => "Plugin Not Found".into(),
            SrvManError::MachineNotFound => "Machine Not Found".into(),
            SrvManError::ConnectionLost => "Connection Lost".into(),
            SrvManError::Shutdown => "Shutdown".into(),
            SrvManError::Err(e) => e.to_string(),
        }
    }
}

impl From<String> for SrvManError {
    fn from(value: String) -> Self {
        SrvManError::Err(failed!(value))
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub enum SrvManInterface {
    Request(SrvManReq),
    Reply(SrvManRep),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum SrvManReq {
    /// Print a text message
    Log(String),

    /// Return a list
    List(SrvManListReq),

    /// Request to upload a source into 'plugins' table
    Upload(Upload),

    /// Request to build and publish if requested in the second entity
    Build(
        PluginId,
        bool,        /* Publish */
        Option<f32>, /* Price */
    ),

    /// Request to load a plugin in a JobSrv
    LoadPlugin(PluginId, bool),

    /// Launch a plugin for a user with given parameters in a JobSrv
    Launch(UserId, Machine),

    /// Subscribe a user to a plugin with the renew option and expiration date
    Subscribe(UserId, PluginId, bool, NaiveDate),

    /// Unsubscribe a user from a plugin
    Unsubscribe(UserId, PluginId),

    /// Stop a running bot (by the id `list bots` shows)
    Stop(RunId),

    /// Shutdown the server
    Shutdown,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum SrvManRep {
    /// Error
    Error(SrvManError),

    /// Todo
    Todo(SrvManReq),

    /// Replay with a list of plugins
    Plugins(HashMap<PluginName, Vec<MachineName>>),

    /// Replay with a list of machines
    Machines(HashMap<MachineName, Machine>),

    /// Replay with a list of parameters
    Params(Vec<Param>),

    /// Replay with a list of clients
    Clients(Vec<String>),

    /// Replay with the registered users
    Users(Vec<crate::db::User>),

    /// Replay with the stored bots
    Bots(Vec<BotInfo>),

    /// Replay received from a CodeSrv
    CodeSrv(JobId, CodeSrvRep),

    /// Replay received from a JobSrv
    JobSrv(JobId, JobSrvRep),

    /// Successfully submitted a job
    Submitted(Vec<JobId>),

    /// Operation was successful
    Successful,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum SrvManListReq {
    /// Return the list of all users
    Users,

    /// Return the list of all connected clients (usually Job Servers)
    Clients,

    /// Return the bots that are running (stored in the database)
    Bots,

    /// Return the list of all loaded plug-ins
    Plugins,

    /// Return the list of all loaded machines
    Machines,

    /// Return the list of machines in a plugin
    PluginMachines(String),

    /// Return the list of a given machine's parameters
    MachineParams(MachineName),
}

impl From<Packet> for SrvManReq {
    fn from(value: Packet) -> Self {
        value.unpack()
    }
}
impl From<Packet> for SrvManRep {
    fn from(value: Packet) -> Self {
        value.unpack()
    }
}

impl From<SrvManError> for SrvManRep {
    fn from(e: SrvManError) -> Self {
        SrvManRep::Error(e)
    }
}

/// A launched machine (a "bot") as stored in the database.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BotInfo {
    pub id: RunId,
    pub user_id: UserId,
    pub plugin_id: PluginId,
    pub plugin_name: PluginName,
    pub machine: MachineName,
    /// The job server it runs on
    pub server: Option<u32>,
}
