use std::net::SocketAddr;

use crate::terminal::{Arg, Args, Executable, Value};

mod codesrv;
mod datasrv;
mod jobsrv;
mod srvman;

use com::net::{Handshake, Packet};
pub use codesrv::*;
pub use datasrv::*;
pub use jobsrv::*;
pub use srvman::*;

#[macro_export]
macro_rules! resprintln {
    () => {
        print!("\n")
    };
    ($($arg:tt)*) => {{
        println!("{}", colored::Colorize::truecolor(format!($($arg)*).as_str(), 128, 128, 128));
    }};
}

#[macro_export]
macro_rules! errprintln {
    () => {
        print!("\n")
    };
    ($($arg:tt)*) => {{
        println!("{}", colored::Colorize::truecolor(format!($($arg)*).as_str(), 128, 0, 0));
    }};
}


#[macro_export]
macro_rules! errprint {
    () => {
        print!()
    };
    ($($arg:tt)*) => {{
        print!("{}", colored::Colorize::truecolor(format!($($arg)*).as_str(), 128, 0, 0));
    }};
}

pub trait CommandInterface {
    fn commands() -> Vec<Executable>;
    fn async_log(_p: Packet, _addr: SocketAddr) {
        unimplemented!()
    }

    fn default_commands() -> Vec<Executable> {
        let mut res = vec![];

        res.push(Executable::new(
            "connect",
            Some("c"),
            "Connect to a server",
            Args::new().add(Arg::new_str("", None)),
            |args, term, _| {
                if let Some((Value::Str(Some(val)), _)) = args.get("") {
                    if term.connected() {
                        // TODO: Prompt disconnect first?
                    }
                    if let Err(e) = term.connect(val) {
                        errprintln!("{e}")
                    } else if let Ok(handshake) = term.receive() {
                        let mut prompt = term.prompt.0.write().unwrap();
                        term.clear_commands();
                        let handshake = Handshake::from(handshake);
                        match &handshake {
                            Handshake::JobSrv(_) => {
                                let _ = term
                                    .intake_commands(JobSrvCI::commands())
                                    .map_err(|s| errprintln!("Warning: {}, skipping", s));
                                term.start_async_recv(JobSrvCI::async_log);
                                *prompt = format!("jobsrv@{}> ", prompt);
                            }
                            Handshake::DataSrv(_) => {
                                let _ = term
                                    .intake_commands(DataSrvCI::commands())
                                    .map_err(|s| errprintln!("Warning: {}, skipping", s));
                                term.start_async_recv(DataSrvCI::async_log);
                                *prompt = format!("datasrv@{}> ", prompt);
                            }
                            Handshake::CodeSrv(_) => {
                                let _ = term
                                    .intake_commands(CodeSrvCI::commands())
                                    .map_err(|s| errprintln!("Warning: {}, skipping", s));
                                term.start_async_recv(CodeSrvCI::async_log);
                                *prompt = format!("codesrv@{}> ", prompt);
                            }
                            Handshake::SrvMan(_) => {
                                let _ = term
                                    .intake_commands(SrvManCI::commands())
                                    .map_err(|s| errprintln!("Warning: {}, skipping", s));
                                term.start_async_recv(SrvManCI::async_log);
                                *prompt = format!("srvman@{}> ", prompt);
                            }
                            Handshake::CLI(_) => unreachable!(),
                        }
                        let _ = term.send(Handshake::CLI(handshake.secret().to_owned()));
                        let _ = term
                            .intake_commands(DataSrvCI::default_commands())
                            .map_err(|s| errprintln!("Warning: {}, skipping", s));
                    }
                } else {
                    errprintln!("Error: missing argument")
                }
                Ok(0)
            },
        ));

        res.push(Executable::new(
            "disconnect",
            Some("d"),
            "Disconnect from the server",
            Args::new(),
            |_, term, _| term.disconnect().map(|_| 0),
        ));

        res.push(Executable::new(
            "help",
            Some("?"),
            "Display all commands",
            Args::new(),
            |_, list, _| {
                list.print_commands();
                Ok(0)
            },
        ));

        res.push(Executable::new(
            "quit",
            Some("q"),
            "Quit the program",
            Args::new(),
            |_, _, _| Ok(-1),
        ));

        return res;
    }
}