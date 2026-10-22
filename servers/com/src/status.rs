use std::fmt::{Debug, Display};

use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Eq, PartialEq)]
pub enum ErrorCode {
    Generic,
    IO,
    FS,
    Socket,
    Network,
    Data,
    Database
}

#[derive(Clone, Serialize, Deserialize, Eq, PartialEq)]
pub struct Error {
    kind: ErrorCode,
    msg: String,
    file: String,
    line: u32,
}

impl Error {
    pub fn what(&self) -> &str {
        self.msg.as_str()
    }
}

impl Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_fmt(format_args!(
            "{:?} Error at {}:{}: {}",
            self.kind, self.file, self.line, self.msg
        ))
    }
}

impl Debug for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self}")
    }
}

#[derive(Clone, Serialize, Deserialize, Eq, PartialEq)]
pub enum Status {
    Successful,
    Failed(Error),
    Multi(Vec<Status>),
}

impl Display for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Status::Successful => f.write_str("Ok"),
            Status::Failed(e) => write!(f, "{e}"),
            Status::Multi(lst) => {
                for e in lst {
                    writeln!(f, "{e}")?
                }
                Ok(())
            }
        }
    }
}

impl Debug for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self}")
    }
}

impl Status {
    pub fn successful() -> Self {
        Self::Successful
    }

    pub fn convert<T, E: ToString>(
        res: Result<T, E>,
        file: &'static str,
        line: u32,
        kind: ErrorCode,
    ) -> Result<T, Self> {
        res.map_err(|e| {
            Self::Failed(Error {
                kind,
                msg: e.to_string(),
                file: file.to_string(),
                line,
            })
        })
    }

    pub fn new<S: ToString>(msg: S, file: &'static str, line: u32, kind: ErrorCode) -> Self {
        Self::Failed(Error {
            kind,
            msg: msg.to_string(),
            file: file.to_string(),
            line,
        })
    }
}

impl From<Error> for Status {
    fn from(e: Error) -> Self {
        Self::Failed(e)
    }
}

#[macro_export]
macro_rules! convert {
    ($e:expr,$k:path) => {
        $crate::status::Status::convert($e, std::file!(), std::line!(), $k)
    };
    ($e:expr) => {
        $crate::status::Status::convert(
            $e,
            std::file!(),
            std::line!(),
            $crate::status::ErrorCode::Generic,
        )
    };
}

#[macro_export]
macro_rules! failed {
    ($e:expr,$k:path) => {
        $crate::status::Status::new($e, std::file!(), std::line!(), $k)
    };
    ($e:expr) => {
        $crate::status::Status::new(
            $e,
            std::file!(),
            std::line!(),
            $crate::status::ErrorCode::Generic,
        )
    };
}


#[macro_export]
macro_rules! err {
    ($e:expr) => {
        format!("{} at {}:{}", $e, std::file!(), std::line!())
    };
}
