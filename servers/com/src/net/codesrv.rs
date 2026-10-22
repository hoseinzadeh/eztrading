use super::*;
use crate::{
    data::Machine,
    db::{self, TxnError}, status::Status, failed,
};
use serde::{Deserialize, Serialize};
use std::fmt::Debug;

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub enum CodeSrvError {
    /// Could not unzip the file maybe because it is broken
    UnzipFailed(Status),

    /// An IO Error
    IoErr(Status),

    /// An IO Error
    DbErr(Status),

    /// A Network Error
    NetErr(Status),

    /// Compilation Error
    CompileError(Status),

    /// Compilation Error
    BuildError(Status),

    /// General Error
    Err(Status),

    /// Shutdown the server
    Shutdown,
}

impl ToString for CodeSrvError {
    fn to_string(&self) -> String {
        match self {
            CodeSrvError::UnzipFailed(e)
            | CodeSrvError::DbErr(e)
            | CodeSrvError::IoErr(e)
            | CodeSrvError::BuildError(e)
            | CodeSrvError::Err(e)
            | CodeSrvError::NetErr(e) => e.to_string(),
            CodeSrvError::CompileError(e) => e.to_string(),
            CodeSrvError::Shutdown => "Shutdown".into(),
        }
    }
}

impl From<String> for CodeSrvError {
    fn from(value: String) -> Self {
        CodeSrvError::Err(failed!(value))
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub enum CodeSrvInterface {
    Request(CodeSrvReq),
    Reply(CodeSrvRep),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum CodeSrvReq {
    /// Print a text message to CodeSrv
    Log(String),

    /// Request to upload a source into 'plugins' table
    Upload(Upload),

    /// Request to build and publish if requested in the second entity
    Build(
        PluginId,
        bool,        /* Publish */
        Option<f32>, /* Price */
        JobId
    ),

    /// Shutdown the server
    Shutdown,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Upload {
    pub id: Option<PluginId>, /* None = new upload */
    pub user: Option<UserId>, /* None => Use PluginId */
    pub name: Option<PluginName>,
    pub source: Option<String>,  /* Source */
    pub version: Option<String>, /* Version */
    pub price: Option<f32>,      /* Price */
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum CodeSrvRep {
    /// Error
    Error(CodeSrvError),

    /// Todo
    Todo(CodeSrvReq),

    /// Compiled library file
    Library(Vec<u8>),

    /// Successfully compiled and uploaded
    Successful(PluginId, PluginName, Vec<Machine>),
}

impl From<Packet> for CodeSrvReq {
    fn from(value: Packet) -> Self {
        value.unpack()
    }
}

impl From<Packet> for CodeSrvRep {
    fn from(value: Packet) -> Self {
        value.unpack()
    }
}

impl From<CodeSrvError> for CodeSrvRep {
    fn from(e: CodeSrvError) -> Self {
        CodeSrvRep::Error(e)
    }
}

impl TxnError for CodeSrvRep {
    fn is_successful(&self) -> bool {
        match self {
            CodeSrvRep::Error(_) => false,
            _ => true,
        }
    }
}

impl db::Error for CodeSrvError {
    fn err<E: ToString>(e: E) -> Self {
        Self::DbErr(failed!(e.to_string()))
    }
}
