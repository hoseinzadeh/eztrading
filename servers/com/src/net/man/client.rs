use std::{
    io::Write,
    sync::{Arc, Mutex},
    thread,
    time::Instant,
};

use crate::{
    convert, failed,
    status::{ErrorCode, Status},
};

/// HeartBeat signal to be sent to the SysMan
pub struct HeartBeat {
    /// The id of the client
    index: usize,

    /// The transmitting channel of the monitoring socket
    tx: crossbeam_channel::Sender<usize>,
}

impl HeartBeat {
    pub fn new(server_ip: &str, index: usize) -> Result<Self, Status> {
        let (tx, rx) = crossbeam_channel::unbounded();
        match crate::connect!(server_ip) {
            Ok(mut srv) => {
                let builder = thread::Builder::new().name("heartbeat-listener".into());
                builder.spawn(move || loop {
                    match rx.recv() {
                        Ok(_) => {
                            if let Err(e) = srv.write_all(&index.to_be_bytes()) {
                                eprintln!("HeartBeat Error: {}", e.to_string());
                                break;
                            }
                        }
                        Err(e) => {
                            eprintln!("HeartBeat Error: {}", e.to_string());
                            break;
                        }
                    }
                }).unwrap();

                // Sending the first beat signal
                convert!(tx.send(index), ErrorCode::Network)?;

                Ok(Self { tx, index })
            }
            Err(e) => Err(failed!(e, ErrorCode::Network)),
        }
    }

    pub fn beat(&self) -> Result<(), Status> {
        convert!(self.tx.send(self.index), ErrorCode::Network)
    }
}

/// HeartBeat signal monitoring system to receive a vital signals from a client
pub struct Monitor {
    /// The timestamp of the last received heartbeat
    #[allow(unused)]
    last: Arc<Mutex<Option<Instant>>>,

    /// SSH arguments to start a new instance of the client
    #[allow(unused)]
    resurrect_args: Vec<String>,
}
