pub mod net;
pub mod data;
pub mod api;
pub mod db;
pub mod ptr;
pub mod status;

extern crate dlopen;
#[macro_use]
extern crate dlopen_derive;

pub const PACKET_SIZE: usize = 60;

pub use mysql;
pub use colored;
pub use chrono;
pub use regex;
pub use libc;

#[inline]
pub fn dylib_file_extension() -> &'static str {
    if std::env::consts::OS == "macos" {
        ".dylib"
    } else {
        ".so"
    }
}