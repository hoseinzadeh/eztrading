use super::CommandInterface;
use crate::{
    errprintln, resprintln,
    terminal::{Arg, Args, Executable, Value},
};
use colored::Colorize;
use com::{
    failed,
    net::{CodeSrvError, CodeSrvRep, CodeSrvReq, Upload},
    status::{ErrorCode, Status}, convert,
};

/// CodeSrv Command Interface
pub struct CodeSrvCI;

impl CommandInterface for CodeSrvCI {
    fn commands() -> Vec<Executable> {
        let mut list = vec![];
        list.push(Executable::new(
            "log",
            None,
            "Send a log string to the server",
            Args::new().add(Arg::new_str("", None)),
            |args, term, _| {
                let mut reqs = vec![];
                for arg in args {
                    if let Value::Str(Some(str)) = &arg.value {
                        reqs.push(CodeSrvReq::Log(str.clone()));
                    }
                }
                for req in reqs {
                    if let Err(Status::Failed(e)) = term.request(req).map(|rep: CodeSrvRep| {
                        resprintln!("{rep:?}");
                        0
                    }) {
                        return Err(failed!(e));
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
                                    term.request(CodeSrvReq::Upload(Upload {
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
                                    .map(|rep: CodeSrvRep| {
                                            if let CodeSrvRep::Error(s) = rep {
                                                errprintln!("{s:?}");
                                            } else {
                                                resprintln!("{rep:?}");
                                            }
                                            0
                                        },
                                    )
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
                Err(failed!("No filename supplied"))
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
                        price = subs.get("--price").map(|(v,_)| v.try_into().unwrap());
                    }
                    
                    term.request(CodeSrvReq::Build(convert!(id.try_into())?, publish, price, 0))
                    .map(|rep: CodeSrvRep| {
                            if let CodeSrvRep::Error(CodeSrvError::CompileError(s)) = rep {
                                errprintln!("{s}");
                            } else if let CodeSrvRep::Successful(id, name, machines) = rep {
                                println!("{}", format!("Plugin '{name}' (id: {id}) was successfully compiled").green());
                                resprintln!("List of machines:");
                                for i in 0..machines.len() {
                                    resprintln!("{:>6}. {}", i+1, machines[i].name.italic());
                                }
                            } else {
                                resprintln!("{rep:?}");
                            }
                            0
                        },
                    )
                } else {
                    Err(failed!("No plugin id supplied"))
                }
            },
        ));

        list.push(Executable::new(
            "shutdown",
            None,
            "Shutdown the server",
            Args::new(),
            |_, term, _| term.request(CodeSrvReq::Shutdown).map(|_: CodeSrvRep| 0),
        ));

        list
    }
}
