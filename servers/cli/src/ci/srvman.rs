use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Arc, Mutex},
};

use chrono::Utc;
use colored::Colorize;
use com::{
    convert,
    data::Machine,
    failed,
    net::{CodeSrvError, CodeSrvRep, Packet, SrvManRep, SrvManReq, Upload},
    status::{ErrorCode, Status},
};
use once_cell::sync::Lazy;

use super::CommandInterface;
use crate::{
    errprintln, resprintln,
    terminal::{Arg, Args, Executable, Value},
};

/// SysMan Command Interface
pub struct SrvManCI;

pub static mut MACHINES: Lazy<Arc<Mutex<HashMap<String, Machine>>>> =
    Lazy::new(|| Arc::new(Mutex::new(HashMap::new())));

impl CommandInterface for SrvManCI {
    fn commands() -> Vec<Executable> {
        let mut list = vec![];

        list.push(Executable::new(
            "list",
            Some("ls"),
            "List the requested items",
            Args::new()
                .add(
                    Arg::new_str("machine", None)
                        .add(Arg::new("--params"))
                        .add(Arg::new_str("--param", None).add(Arg::new("--options"))),
                )
                .add(Arg::new_str("plugin", None))
                .add(Arg::new("plugins"))
                .add(Arg::new("bots"))
                .add(Arg::new("machines"))
                .add(Arg::new("users"))
                .add(Arg::new("servers")),
            |args, term, _| {
                if args.len() != 1 {
                    Err(failed!(format!(
                        "expected 1 argument, found {}",
                        args.len()
                    )))
                } else {
                    use com::net::{SrvManListReq::*, SrvManReq::*};
                    match args[0].name.as_str() {
                        "plugins" => term.request(List(Plugins)),
                        "bots" => term.request(List(Bots)),
                        "machines" => term.request(List(Machines)),
                        "users" => term.request(List(Users)),
                        "servers" => term.request(List(Clients)),
                        "plugin" => {
                            if let Value::Str(Some(plugin_name)) = &args[0].value {
                                term.request(List(PluginMachines(plugin_name.clone())))
                            } else {
                                Err(failed!("no plugin name is provided"))
                            }
                        }
                        "machine" => {
                            if let Value::Str(Some(machine_name)) = &args[0].value {
                                term.request(List(MachineParams(machine_name.clone())))
                            } else {
                                Err(failed!("no machine name is provided"))
                            }
                        }
                        _ => Err(Status::Successful),
                    }
                    .map(|rep: SrvManRep| {
                        resprintln!("{rep:#?}");
                        0
                    })
                }
            },
        ));

        list.push(Executable::new(
            "log",
            None,
            "Send a log string to the server",
            Args::new().add(Arg::new_str("", None)),
            |args, term, _| {
                let mut reqs = vec![];
                for arg in args {
                    if let Value::Str(Some(str)) = &arg.value {
                        reqs.push(SrvManReq::Log(str.clone()));
                    }
                }
                for req in reqs {
                    if let Err(Status::Failed(e)) = term.request(req).map(|rep: SrvManRep| {
                        resprintln!("{rep:?}");
                        0
                    }) {
                        errprintln!("{}", e);
                    }
                }
                Ok(0)
            },
        ));

        list.push(Executable::new(
            "upload",
            Some("u"),
            "Upload an SML file to the server (e.g., simple.sm)",
            Args::new().add(
                Arg::new_str("", None)
                    .add(Arg::new("--publish"))
                    .add(Arg::new("--rebuild"))
                    .add(Arg::new_int("--user", None))
                    .add(Arg::new_int("--id", None))
                    .add(Arg::new_str("--name", None))
                    .add(Arg::new_float("--price", None))
                    .add(Arg::new_str("--version", Some("1.0.0"))),
            ),
            |args, term, _| {
                use std::fs::File;
                use std::io::Read;
                for arg in args {
                    if arg.name.is_empty() {
                        let sub = &arg.args;
                        if let Value::Str(Some(filename)) = &arg.value {
                            return if let Ok(mut f) = File::open(&filename) {
                                let mut buffer = String::new();
                                if let Ok(_) = f.read_to_string(&mut buffer) {
                                    let mut path = std::path::PathBuf::from(filename);
                                    while path.extension().is_some() {
                                        path.set_extension("");
                                    }
                                    let plugin_name = if let Some((Value::Str(val), _)) =
                                        sub.get("--name")
                                    {
                                        val.clone()
                                    } else if sub.get("--id").is_none() {
                                        Some(path.file_name().unwrap().to_str().unwrap().to_owned())
                                    } else {
                                        None
                                    };
                                    term.request(SrvManReq::Upload(Upload {
                                        id: sub
                                            .get("--id")
                                            .map(|(v, _)| v.clone().try_into().ok())
                                            .unwrap_or_default(),
                                        user: sub
                                            .get("--user")
                                            .map(|(v, _)| v.clone().try_into().ok())
                                            .unwrap_or_default(),
                                        name: plugin_name,
                                        source: Some(buffer),
                                        version: sub
                                            .get("--version")
                                            .map(|(v, _)| v.try_into().ok())
                                            .unwrap_or_default(),
                                        price: sub
                                            .get("--price")
                                            .map(|(v, _)| v.clone().try_into().ok())
                                            .unwrap_or_default(),
                                    }))
                                    .map(|rep: SrvManRep| {
                                        if let SrvManRep::CodeSrv(
                                            job_id,
                                            CodeSrvRep::Error(CodeSrvError::CompileError(s)),
                                        ) = rep
                                        {
                                            errprintln!("Job #{job_id}: {s}");
                                        } else if let SrvManRep::CodeSrv(job_id, rep) = rep {
                                            resprintln!("Job #{job_id}: {rep:#?}");
                                        } else {
                                            resprintln!("{rep:#?}");
                                        }
                                        0
                                    })
                                } else {
                                    Err(failed!("Could not read file", ErrorCode::FS))
                                }
                            } else {
                                Err(failed!(
                                    format!("No such file as '{}'", filename),
                                    ErrorCode::FS
                                ))
                            };
                        }
                    }
                }
                Err(failed!("No filename is supplied"))
            },
        ));

        list.push(Executable::new(
            "build",
            Some("b"),
            "Build a plugin given it's id",
            Args::new().add(
                Arg::new_int("", None)
                    .add(Arg::new("--publish").add(Arg::new_float("--price", None))),
            ),
            |args, term, _| {
                if let Some((id, args)) = args.get("") {
                    let mut price: Option<f32> = None;
                    let mut publish = false;
                    if let Some((_, subs)) = args.get("--publish") {
                        publish = true;
                        price = subs.get("--price").map(|(v, _)| v.try_into().unwrap());
                    }

                    term.request(SrvManReq::Build(convert!(id.try_into())?, publish, price))
                        .map(|rep: SrvManRep| {
                            if let SrvManRep::Submitted(jobs) = rep {
                                resprintln!(
                                    "Job(s) #{:?}: {}",
                                    jobs,
                                    format!("Successfully submitted to the code server").green()
                                );
                            } else {
                                resprintln!("{rep:#?}");
                            }
                            0
                        })
                } else {
                    Err(failed!("No plugin id supplied"))
                }
            },
        ));

        list.push(Executable::new(
            "load",
            Some("ld"),
            "Load plugins into JobSrv(s)",
            Args::new().add(Arg::new_int("", None).add(Arg::new("--force"))),
            |args, term, _| {
                if args.len() != 1 {
                    Err(failed!(format!(
                        "expected 1 argument, found {}",
                        args.len()
                    )))
                } else if let Some((id, args)) = args.get("") {
                    let mut force = false;
                    if args.get("--force").is_some() {
                        force = true;
                    }

                    term.request(SrvManReq::LoadPlugin(convert!(id.try_into())?, force))
                        .map(|rep: SrvManRep| {
                            if let SrvManRep::Submitted(jobs) = rep {
                                resprintln!(
                                    "Job(s) #{:?}: {}",
                                    jobs,
                                    format!("Successfully submitted to the job servers").green()
                                );
                            } else {
                                resprintln!("{rep:#?}");
                            }
                            0
                        })
                } else {
                    Err(failed!("No plugin id supplied"))
                }
            },
        ));

        list.push(Executable::new(
            "launch",
            Some("l"),
            "Launch a machine for a given user",
            Args::new()
                .add(Arg::new_str("", None))
                .add(Arg::new_int("--user", None))
                .add(Arg::new_str("--set", None)),
            |args, term, _| {
                if let Some((Value::Str(Some(m)), _)) = args.get("") {
                    if let Some((Value::Int(Some(u)), _)) = args.get("--user") {
                        if let Some(m) = unsafe { MACHINES.lock() }.unwrap().get(m) {
                            let mut m = m.clone();
                            // --set "equities=AAPL,MSFT invest=500"
                            if let Some((Value::Str(Some(sets)), _)) = args.get("--set") {
                                for pair in sets.split_whitespace() {
                                    let Some((k, v)) = pair.split_once('=') else {
                                        return Err(failed!(format!("expected name=value, found `{pair}`")));
                                    };
                                    match m.params.iter_mut().find(|p| p.id == k) {
                                        Some(p) => p.set_value(v).map_err(|e| failed!(format!("{k}: {e}")))?,
                                        None => return Err(failed!(format!("the machine has no parameter `{k}`"))),
                                    }
                                }
                            }
                            if let Err(Status::Failed(e)) = term
                                .request(SrvManReq::Launch(*u as u32, m))
                                .map(|rep: SrvManRep| {
                                    if let SrvManRep::Submitted(jobs) = rep {
                                        resprintln!(
                                            "Job(s) #{:?}: {}",
                                            jobs,
                                            format!("Successfully submitted to the job server(s)")
                                                .green()
                                        );
                                    } else {
                                        resprintln!("{rep:#?}");
                                    }
                                    0
                                })
                            {
                                Err(failed!(e))
                            } else {
                                Ok(0)
                            }
                        } else {
                            Err(failed!("Unknown machine name"))
                        }
                    } else {
                        Err(failed!("No user id is provided"))
                    }
                } else {
                    Err(failed!("No machine name is provided"))
                }
            },
        ));

        list.push(Executable::new(
            "stop",
            None,
            "Stop a running bot (see `list bots`)",
            Args::new().add(Arg::new_int("", None)),
            |args, term, _| {
                if let Some((id, _)) = args.get("") {
                    term.request(SrvManReq::Stop(convert!(id.try_into())?)).map(|rep: SrvManRep| {
                        resprintln!("{rep:?}");
                        0
                    })
                } else {
                    Err(failed!("No bot id supplied"))
                }
            },
        ));

        list.push(Executable::new(
            "subscribe",
            Some("sub"),
            "Subscribe a user to a plugin",
            Args::new()
                .add(Arg::new_int("--user", None))
                .add(Arg::new_int("--plugin", None))
                .add(Arg::new("--renew"))
                .add(Arg::new_int("--expire", None)),
            |args, term, _| {
                if let Some((user_id, _)) = args.get("--user") {
                    if let Some((plugin_id, _)) = args.get("--plugin") {
                        let expire_days = args
                            .get("--expire")
                            .map(|(v, _)| v.try_into().unwrap())
                            .unwrap_or(30);
                        let expire = Utc::now() + chrono::Duration::days(expire_days);
                        let renew = args.get("--renew").is_some();
                        term.request(SrvManReq::Subscribe(
                            convert!(user_id.try_into())?,
                            convert!(plugin_id.try_into())?,
                            renew,
                            expire.date_naive(),
                        ))
                        .map(|rep: SrvManRep| {
                            resprintln!("{rep:?}");
                            0
                        })
                    } else {
                        Err(failed!("No plugin id is supplied"))
                    }
                } else {
                    Err(failed!("No user id supplied"))
                }
            },
        ));

        list.push(Executable::new(
            "unsubscribe",
            Some("usub"),
            "Unsubscribe a user from a plugin",
            Args::new()
                .add(Arg::new_int("--user", None))
                .add(Arg::new_int("--plugin", None)),
            |args, term, _| {
                if let Some((user_id, _)) = args.get("--user") {
                    if let Some((plugin_id, _)) = args.get("--plugin") {
                        term.request(SrvManReq::Unsubscribe(
                            convert!(user_id.try_into())?,
                            convert!(plugin_id.try_into())?
                        ))
                        .map(|rep: SrvManRep| {
                            resprintln!("{rep:?}");
                            0
                        })
                    } else {
                        Err(failed!("No plugin id is supplied"))
                    }
                } else {
                    Err(failed!("No user id supplied"))
                }
            },
        ));

        list.push(Executable::new(
            "shutdown",
            None,
            "Shutdown the server",
            Args::new(),
            |_, term, _| term.request(SrvManReq::Shutdown).map(|_: SrvManRep| 0),
        ));

        list
    }

    fn async_log(p: Packet, _addr: SocketAddr) {
        use com::net::JobSrvRep;
        let rep = SrvManRep::from(p);
        match &rep {
            SrvManRep::CodeSrv(job_id, CodeSrvRep::Error(CodeSrvError::CompileError(s))) => {
                errprintln!("\nJob #{job_id}: {s}")
            }
            SrvManRep::CodeSrv(job_id, CodeSrvRep::Successful(id, name, machines)) => {
                resprintln!(
                    "\nJob #{job_id}: {}",
                    format!("Plugin '{name}' (id: {id}) was successfully compiled").green()
                );
                resprintln!("List of machines:");
                for i in 0..machines.len() {
                    resprintln!("{:>6}. {}", i + 1, machines[i].name.italic());
                }
            }
            SrvManRep::CodeSrv(job_id, rep) => resprintln!("\nJob #{job_id}: {rep:#?}"),
            SrvManRep::JobSrv(job_id, JobSrvRep::Plugin(_, m)) => unsafe {
                let mut machines = MACHINES.lock().unwrap();
                for m in m {
                    machines.insert(m.name.clone(), m.clone());
                }
                resprintln!(
                    "\nJob #{job_id}: The following machines are added:\n   {}",
                    m.iter()
                        .map(|m| m.name.as_str())
                        .collect::<Vec<&str>>()
                        .join("\n   ")
                );
            },
            SrvManRep::JobSrv(job_id, rep) => resprintln!("\nJob #{job_id}: {rep:#?}"),
            _ => resprintln!("\n{rep:#?}"),
        }
    }
}
