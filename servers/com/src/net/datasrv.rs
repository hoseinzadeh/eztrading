use super::{packet::Packet, *};
use crate::{data::{Machine, Param, Ticker, search::{Filter}}, status::Status, failed};
use serde::{Deserialize, Serialize};
use std::{fmt::Debug, collections::HashMap};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum DataSrvError {
    Shutdown,
    Err(Status),
}

impl ToString for DataSrvError {
    fn to_string(&self) -> String {
        match self {
            DataSrvError::Shutdown => "Shutdown".into(),
            DataSrvError::Err(e) => e.to_string(),
        }
    }
}

impl From<String> for DataSrvError {
    fn from(value: String) -> Self {
        DataSrvError::Err(failed!(value))
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub enum DataSrvInterface {
    Request(DataSrvReq),
    Reply(DataSrvRep),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum DataSrvReq {
    /// Print a text message to DataSrv
    Log(String),

    /// Return a list
    List(DataSrvListReq),

    /// Register the requestor as a listener to the given symbols
    Listen(Vec<Symbol>),

    /// Send the time-series for the given symbols defined in the filter
    TimeSeries(Filter),

    /// Send the earliest time-stamp for the given symbols defined in the filter
    EarliestTimestamp(Filter),

    /// Request HeartBeat
    HeartBeat,

    /// Shutdown the server
    Shutdown,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum DataSrvRep {
    /// Replay with a list of plugins
    Plugins(Vec<FilePath>),

    /// Replay with a list of machines
    Machines(Vec<Machine>),

    /// Replay with a list of parameters
    Params(Vec<Param>),

    /// Replay with a list of clients
    Clients(Vec<String>),

    /// TimeSeries
    TimeSeries(HashMap<Symbol, Vec<Ticker>>),
    
    /// Earliest timestamp
    EarliestTimestamp(HashMap<Symbol, Result<i64,String>>),

    /// Error
    Error(DataSrvError),

    /// Todo
    Todo(DataSrvReq),

    /// HeartBeat
    HeartBeat,

    /// Operation was successful
    Successful,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum DataSrvListReq {
    /// Return the list of all users
    Users,

    /// Return the list of all connected clients (usually Job Servers)
    Clients,
}

impl From<Packet> for DataSrvReq {
    fn from(value: Packet) -> Self {
        value.unpack()
    }
}

impl From<Packet> for DataSrvRep {
    fn from(value: Packet) -> Self {
        value.unpack()
    }
}

impl From<DataSrvError> for DataSrvRep {
    fn from(e: DataSrvError) -> Self {
        DataSrvRep::Error(e)
    }
}
