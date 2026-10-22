use std::str::FromStr;
use colored::Colorize;
use com::{
    data::Options,
    failed,
    net::{JobSrvListReq, JobSrvRep, JobSrvReq},
    status::Status,
};
use super::CommandInterface;
use crate::{
    errprintln, resprintln,
    terminal::{Arg, Args, Executable, Value}, ci::MACHINES,
};

/// JobSrv Command Interface
pub struct JobSrvCI;

impl CommandInterface for JobSrvCI {
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
                .add(Arg::new_int("plugin", None))
                .add(Arg::new("machines").add(Arg::new("--params")))
                .add(Arg::new("plugins")
                    .add(Arg::new("--all")
                        .add(Arg::new("--pub"))
                        .add(Arg::new_str("--cols", Some("id,name,owner,owner_id,owner_email,version,price,published,date,machines")))
                    )
                )
                .add(Arg::new("schedulers")),
            |args, term, _| {
                if args.len() != 1 {
                    Err(failed!(format!(
                        "expected 1 argument, found {}",
                        args.len()
                    )))
                } else {
                    match args[0].name.as_str() {
                        "machine" => {
                            if let Some((Value::Str(Some(mname)), sub)) = args.get("machine") {
                                term.request(JobSrvReq::List(JobSrvListReq::Machines)).map(
                                    |rep: JobSrvRep| {
                                        if let JobSrvRep::Machines(machines) = rep {
                                            if let Some(machine) = machines.get(mname) {
                                                if let Some((Value::Str(Some(pid)), sub)) =
                                                    sub.get("--param")
                                                {
                                                    let mut found = false;
                                                    for p in &machine.params {
                                                        if p.id == pid.as_str() {
                                                            if sub.get("--options").is_some() {
                                                                if let Options::List(lst) =
                                                                    &p.options
                                                                {
                                                                    for o in lst {
                                                                        println!(
                                                                            "  {}",
                                                                            o.name.blue()
                                                                        )
                                                                    }
                                                                } else {
                                                                    println!("{:?}", p.options)
                                                                }
                                                            } else {
                                                                println!("{:#?}", p);
                                                            }
                                                            found = true;
                                                            break;
                                                        }
                                                    }
                                                    if !found {
                                                        errprintln!("Unknown parameter '{pid}'");
                                                    }
                                                } else if let Some(_) = sub.get("--params") {
                                                    for p in &machine.params {
                                                        println!(
                                                            "  {} ({}): {}",
                                                            p.id.italic(),
                                                            p.name.green(),
                                                            p.r#type.magenta()
                                                        )
                                                    }
                                                } else {
                                                    println!("{:#?}", machine);
                                                }
                                            } else {
                                                errprintln!("No such machine as '{}'", mname)
                                            }
                                        }
                                        0
                                    },
                                )
                            } else {
                                Err(failed!("No machine name is provided"))
                            }
                        }
                        "plugin" => {
                            if let Some((Value::Int(Some(id)), _)) = args.get("plugin") {
                                term.request(JobSrvReq::List(JobSrvListReq::PluginMachines(
                                    *id as u32,
                                )))
                                .map(|rep: JobSrvRep| {
                                    println!("{:#?}", rep);
                                    0
                                })
                            } else {
                                Err(failed!("No plugin name is provided"))
                            }
                        }
                        "machines" => term.request(JobSrvReq::List(JobSrvListReq::Machines)).map(
                            |rep: JobSrvRep| {
                                if let JobSrvRep::Machines(machines) = rep {
                                    let params = args[0].args.get("--params").is_some();
                                    for (mname, machine) in machines {
                                        println!("  {}", mname.blue());
                                        if params {
                                            for p in &machine.params {
                                                println!(
                                                    "     {} ({}): {}",
                                                    p.id.italic(),
                                                    p.name.green(),
                                                    p.r#type.magenta()
                                                )
                                            }
                                        }
                                    }
                                }
                                0
                            },
                        ),
                        "plugins" => {
                            if let Some((_, sub)) = args[0].args.get("--all") {
                                let cols: &str = sub.get("--cols").unwrap().0.try_into().unwrap();
                                let cols: Vec<&str> = cols.split(",").map(|v| v.trim()).collect();
                                let only_published = sub.get("--pub").is_some();
                                term.request(JobSrvReq::List(JobSrvListReq::AllPlugins)).map(
                                    |rep: JobSrvRep| {
                                        if let JobSrvRep::AllPlugins(all) = rep {
                                            let mut out = "{\"plugins\": [".to_owned();
                                            let show_id = cols.contains(&"id");
                                            let show_name = cols.contains(&"name");
                                            let show_owner = cols.contains(&"owner");
                                            let show_owner_id = cols.contains(&"owner_id");
                                            let show_owner_email = cols.contains(&"owner_email");
                                            let show_version = cols.contains(&"version");
                                            let show_price = cols.contains(&"price");
                                            let show_published = cols.contains(&"published");
                                            let show_date = cols.contains(&"date");
                                            let show_machines = cols.contains(&"machines");
                                            
                                            for (id,p) in all {
                                                if only_published && !p.published { continue; }
                                                out += "{";
                                                if show_id { out += &format!("\"id\": {}, ", id) }
                                                if show_name { out += &format!("\"name\": \"{}\", ", p.name.replace("\"", "\\\"")) }
                                                if show_owner { out += &format!("\"owner\": \"{}\", ", p.owner_name.replace("\"", "\\\"")) }
                                                if show_owner_id { out += &format!("\"owner_id\": {}, ", p.owner_id) }
                                                if show_owner_email { out += &format!("\"owner_email\": \"{}\", ", p.owner_email.replace("\"", "\\\"")) }
                                                if show_version { out += &format!("\"version\": \"{}\", ", p.version) }
                                                if show_price { if let Some(p) = p.price { out += &format!("\"price\": {p}, ") }}
                                                if show_date { if let Some(d) = p.date { out += &format!("\"date\": \"{d}\", ") }}
                                                if show_published { out += &format!("\"published\": {}, ", p.published) }
                                                if show_machines { out += &format!("\"machines\": {}, ", serde_json::to_string(&p.machines).unwrap()) }
                                                if let Some(pos) = out.rfind(", ") {
                                                    let _ = out.split_off(pos);
                                                }
                                                out += "}, ";
                                            }
                                            if let Some(pos) = out.rfind(", ") {
                                                let _ = out.split_off(pos);
                                            }
                                            out += "]}";
                                            let val = serde_json::Value::from_str(&out).unwrap();
                                            resprintln!("{}", serde_json::to_string_pretty(&val).unwrap());
                                        } else {
                                            resprintln!("{rep:#?}");
                                        }
                                        0
                                    },
                                )
                            } else {
                                term.request(JobSrvReq::List(JobSrvListReq::Plugins)).map(
                                    |rep: JobSrvRep| {
                                        println!("{:#?}", rep);
                                        0
                                    },
                                )
                            }
                        }
                        _ => Err(Status::Successful),
                    }
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
                        reqs.push(JobSrvReq::Log(str.clone()));
                    }
                }
                for req in reqs {
                    if let Err(Status::Failed(e)) = term.request(req).map(|rep: JobSrvRep| {
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
            "listen",
            Some("ln"),
            "Start listening to the price of the given symbols",
            Args::new().add(Arg::new_str("", None)),
            |args, term, _| {
                let mut symbols = vec![];
                for arg in args {
                    if let Value::Str(Some(str)) = &arg.value {
                        symbols.push(str.clone());
                    }
                }
                if let Err(Status::Failed(e)) =
                    term.request(JobSrvReq::Listen(symbols))
                        .map(|rep: JobSrvRep| {
                            resprintln!("{rep:?}");
                            0
                        })
                {
                    Err(failed!(e))
                } else {
                    Ok(0)
                }
            },
        ));

        list.push(Executable::new(
            "load",
            None,
            "Load a plugins given their ids",
            Args::new().add(Arg::new_int("", None)),
            |args, term, _| {
                let mut reqs = vec![];
                for arg in args {
                    if let Value::Int(Some(id)) = &arg.value {
                        reqs.push(JobSrvReq::LoadPlugin(*id as u32, false));
                    }
                }
                for req in reqs {
                    if let Err(Status::Failed(e)) = term.request(req).map(|rep: JobSrvRep| {
                        if let JobSrvRep::Plugin(_, m) = &rep {
                            unsafe {
                                let mut machines = MACHINES.lock().unwrap();
                                for m in m {
                                    machines.insert(m.name.clone(), m.clone());
                                }
                                resprintln!(
                                    "The following machines are added:\n   {}",
                                    m.iter()
                                        .map(|m| m.name.as_str())
                                        .collect::<Vec<&str>>()
                                        .join("\n   ")
                                );
                            }
                        } else {
                            resprintln!("{rep:#?}");
                        }
                        0
                    }) {
                        errprintln!("{}", e);
                    }
                }
                Ok(0)
            },
        ));

        list.push(Executable::new(
            "launch",
            Some("l"),
            "Launch a machine",
            Args::new().add(Arg::new_str("", None)).add(Arg::new_int("--user", None)),
            |args, term, _| {
                if let Some((Value::Str(Some(m)), _)) = args.get("") {
                    if let Some((Value::Int(Some(u)), _)) = args.get("--user") {
                        if let Some(m) = unsafe { MACHINES.lock() }.unwrap().get(m) {
                            if let Err(Status::Failed(e)) = term.request(JobSrvReq::Launch(*u as u32, m.clone())).map(|rep: JobSrvRep| {
                                resprintln!("{rep:#?}");
                                0
                            }) {
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
            "shutdown",
            None,
            "Shutdown the server",
            Args::new(),
            |_, term, _| term.request(JobSrvReq::Shutdown).map(|_: JobSrvRep| 0),
        ));
        list
    }
}
