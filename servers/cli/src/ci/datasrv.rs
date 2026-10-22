use std::{collections::HashMap, sync::atomic::AtomicBool, time::Duration, thread};
use super::CommandInterface;
use crate::{
    errprintln, resprintln,
    terminal::{Arg, Args, Executable, Value},
};
use chrono::NaiveDateTime;
use com::{
    data::search::{Filter, Interval},
    failed,
    net::{DataSrvError},
    status::Status, convert,
};
use once_cell::sync::Lazy;

/// DataSrv Command Interface
pub struct DataSrvCI;

impl CommandInterface for DataSrvCI {
    fn commands() -> Vec<Executable> {
        use com::net::{DataSrvRep, DataSrvReq};

        let mut list = vec![];
        list.push(Executable::new(
            "list",
            Some("ls"),
            "List the requested items",
            Args::new()
                .add(Arg::new("users"))
                .add(Arg::new("clients")),
            |args, term, _| {
                if args.len() != 1 {
                    Err(failed!(format!(
                        "expected 1 argument, found {}",
                        args.len()
                    )))
                } else {
                    use com::net::{DataSrvReq::*, DataSrvListReq::*};
                    match args[0].name.as_str() {
                        "users" => term.request(List(Users)),
                        "clients" => term.request(List(Clients)),
                        _ => Err(Status::Successful),
                    }
                    .map(|rep: com::net::DataSrvRep| {
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
                        reqs.push(DataSrvReq::Log(str.clone()));
                    }
                }
                for req in reqs {
                    if let Err(Status::Failed(e)) = term.request(req).map(|rep: DataSrvRep| {
                        resprintln!("{rep:?}");
                        0
                    }) {
                        return Err(failed!(format!("{e}")));
                    }
                }
                Ok(0)
            },
        ));

        let filters = Args::new()
            .add(Arg::new_str("", None))
            .add(Arg::new_str("--interval", None))
            .add(Arg::new_str("--country", None))
            .add(Arg::new_str("--mic", None))
            .add(Arg::new_str("--start", None))
            .add(Arg::new_str("--end", None));

        fn parse_filters(args: &Args) -> Result<Filter, Status> {
            let mut filter = Filter::new();
            for arg in args {
                match arg.name.as_str() {
                    "--interval" => {
                        if let Value::Str(Some(si)) = &arg.value {
                            if let Some(i) = Interval::from_str(si) {
                                filter = filter.interval(i)
                            } else {
                                return Err(failed!(format!("Unknown interval `{}` (available values ar '1min', '5min', '15min', \
                                '30min', '45min', '1h', '2h', '4h', 'daily', 'weekly', and 'monthly')", si)));
                            }
                        }
                    }
                    "--start" => {
                        if let Value::Str(Some(si)) = &arg.value {
                            if let Ok(t) = NaiveDateTime::parse_from_str(si, "%Y-%m-%d %H:%M:%S") {
                                filter = filter.start(t.timestamp())
                            } else if let Ok(t) = NaiveDateTime::parse_from_str(
                                format!("{si} 00:00:00").as_str(),
                                "%Y-%m-%d %H:%M:%S",
                            ) {
                                filter = filter.start(t.timestamp())
                            } else {
                                return Err(failed!(format!("Invalid date-time format for `{}` (acceptable formats are 2006-01-02 or 2006-01-02 15:04:05)", si)));
                            }
                        }
                    }
                    "--end" => {
                        if let Value::Str(Some(si)) = &arg.value {
                            if let Ok(t) = NaiveDateTime::parse_from_str(si, "%Y-%m-%d %H:%M:%S") {
                                filter = filter.end(t.timestamp())
                            } else if let Ok(t) = NaiveDateTime::parse_from_str(
                                format!("{si} 00:00:00").as_str(),
                                "%Y-%m-%d %H:%M:%S",
                            ) {
                                filter = filter.end(t.timestamp())
                            } else {
                                return Err(failed!(format!("Invalid date-time format for `{}` (acceptable formats are 2006-01-02 or 2006-01-02 15:04:05)", si)));
                            }
                        }
                    }
                    _ => (),
                }
            }
            let mut symbols = vec![];
            for arg in args {
                if let Value::Str(Some(str)) = &arg.value {
                    if arg.name.is_empty() {
                        symbols.push(str.clone());
                    }
                }
            }
            Ok(filter.symbols(&symbols))
        }

        list.push(Executable::new(
            "time-series",
            Some("ts"),
            "Request the time series of the given symbol",
            filters.clone(),
            |args, term, _| {
                if let Err(Status::Failed(e)) = term
                    .request(DataSrvReq::TimeSeries(parse_filters(args)?))
                    .map(|rep: DataSrvRep| {
                        match rep {
                            DataSrvRep::TimeSeries(list) => resprintln!("{list:#?}"),
                            DataSrvRep::Error(DataSrvError::Err(e)) => errprintln!("{e}"),
                            _ => unreachable!(),
                        }
                        0
                    })
                {
                    return Err(failed!(format!("{e}")));
                }
                Ok(0)
            },
        ));

        list.push(Executable::new(
            "parallel-time-series",
            Some("pts"),
            "Send 100 requests for time series of the given symbol, in parallel",
            filters.clone().add(Arg::new_int("--threads", None)),
            |args, term, _| {
                let mut thrds = vec![];

                if let Some((val, _)) = args.get("--threads") {
                    if let Value::Int(Some(t)) = val {
                        for i in 0..*t {
                            let args = args.clone();
                            let term = term.clone();
                            let builder = thread::Builder::new().name(format!("time-series-{i}"));
                            thrds.push(builder.spawn(move || {
                                println!("[{}] New Req sent", chrono::Local::now());
                                let _ = term
                                    .request(DataSrvReq::TimeSeries(parse_filters(&args).unwrap()))
                                    .map(|rep: DataSrvRep| {
                                        match rep {
                                            DataSrvRep::TimeSeries(list) => resprintln!("{list:#?}"),
                                            DataSrvRep::Error(DataSrvError::Err(e)) => errprintln!("{e}"),
                                            _ => () //unreachable!(),
                                        }
                                        0
                                    });
                            }).unwrap());
                        }
                        while let Some(t) = thrds.pop() {
                            t.join().unwrap();
                        }
                        Ok(0)
                    } else {
                        Err(failed!("Invalid --threads parameter"))
                    }
                } else {
                    Err(failed!("Argument --threads is required"))
                }
            },
        ));

        list.push(Executable::new(
            "earliest-timestamp",
            Some("ets"),
            "Request the earliest timestamp of the given symbol",
            filters.clone(),
            |args, term, _| {
                if let Err(Status::Failed(e)) = term
                    .request(DataSrvReq::EarliestTimestamp(parse_filters(args)?))
                    .map(|rep: DataSrvRep| {
                        match rep {
                            DataSrvRep::EarliestTimestamp(list) => resprintln!(
                                "{:#?}",
                                list.into_iter()
                                    .map(|(k, v)| (
                                        k,
                                        match v.map(|t| NaiveDateTime::from_timestamp_opt(t, 0)
                                            .map(|v| v.to_string())
                                            .unwrap_or("Failed to convert".to_owned()))
                                        {
                                            Ok(val) => val,
                                            Err(e) => e,
                                        }
                                    ))
                                    .collect::<HashMap<String, String>>()
                            ),
                            DataSrvRep::Error(DataSrvError::Err(e)) => errprintln!("{e}"),
                            _ => unreachable!(),
                        }
                        0
                    })
                {
                    return Err(failed!(format!("{e}")));
                }
                Ok(0)
            },
        ));


        list.push(Executable::new(
            "websocket",
            Some("ws"),
            "Create a HTTP server and connect to the DataSrv WebSocket for prices",
            Args::new().add(Arg::new_str("", None))
                .add(Arg::new("open"))
                .add(Arg::new("close"))
                .add(Arg::new_str("--url", Some("127.0.0.1:8080")))
                .add(Arg::new_str("--key", None))
                .add(Arg::new_str("--http", Some("127.0.0.1:8080")))
                .add(Arg::new_str("--symbols", None))
                .add(Arg::new("list")),
            |args, _term, cmd| {
                use std::net::TcpStream;
                use std::io::Write;
                use std::sync::atomic::Ordering::Relaxed;

                let mut symbols = vec![];
                for arg in args {
                    if let Value::Str(Some(str)) = &arg.value {
                        if arg.name.is_empty() {
                            symbols.push(str.clone());
                        }
                    }
                }
                
                fn handle_write(mut stream: TcpStream) {
                    let html = std::fs::read_to_string("assets/cli/ws.html").unwrap();
                    let response = format!("HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=UTF-8\r\n\r\n{html}\r\n");
                    match stream.write(response.as_bytes()) {
                        Ok(_) => (),
                        Err(e) => errprintln!("Failed sending response: {}", e),
                    }
                }
                
                fn handle_client(stream: TcpStream) {
                    handle_write(stream);
                }
                
                static mut THREADS_WORKING: Lazy<HashMap<String, AtomicBool>> = Lazy::new(|| HashMap::new());

                let mut open = false;
                let mut close = false;
                let mut list = false;
                let mut symbols: Vec<String> = vec![];
                let mut addr = "127.0.0.1:8080".to_owned();
                for arg in args {
                    match arg.name.as_str() {
                        "open" => open = true,
                        "close" => close = true,
                        "list" => list = true,
                        "--http" => if let Value::Str(Some(a)) = &arg.value {
                            addr = a.clone();
                        },
                        "--symbols" => if let Value::Str(Some(syms)) = &arg.value {
                            symbols.append(&mut syms.split(',').map(|v| v.trim().to_owned()).collect())
                        },
                        _ => {
                            errprintln!("Invalid input");
                            cmd.help();
                        }
                    }
                }

                if open && close {
                    Err(failed!("Cannot open and close a http server at the same time"))
                } else if open {
                    let listener = com::listen!(&addr)?;
                    convert!(listener.set_nonblocking(true))?;
                    resprintln!("Listening for connections on address {}", addr);

                    let key = addr.clone();
                    let builder = thread::Builder::new().name("ws-http-server".into());
                    builder.spawn(move || {
                        unsafe { THREADS_WORKING.entry(key.clone()).or_insert_with(|| AtomicBool::new(true)); }
                        while unsafe {THREADS_WORKING.get(&key)}.unwrap().load(Relaxed) {
                            match listener.accept() {
                                Ok((stream, client)) => {
                                    resprintln!("New connection from {client} to {key}");
                                    let builder = thread::Builder::new().name(format!("ws-http-client-{client}"));
                                    builder.spawn(|| {
                                        handle_client(stream)
                                    }).unwrap();
                                }
                                Err(_) => {
                                    thread::sleep(Duration::from_millis(100))
                                }
                            }
                        }
                        unsafe {THREADS_WORKING.remove(&key) };
                    }).unwrap();

                    Ok(0)
                } else if close {
                    match unsafe {THREADS_WORKING.get(&addr)} {
                        Some(working) => {
                            working.store(false, Relaxed);
                        }
                        None => errprintln!("Invalid socket address"),
                    }
                    Ok(0)
                } else if list {
                    resprintln!("{}", unsafe {THREADS_WORKING.keys().clone().map(|v| v.as_str()).collect::<Vec<&str>>().join("\n")});
                    Ok(0)
                } else {
                    cmd.help();
                    Ok(0)
                }
            },
        ));

        list.push(Executable::new(
            "shutdown",
            None,
            "Shutdown the server",
            Args::new(),
            |_, term, _| term.request(DataSrvReq::Shutdown).map(|_: DataSrvRep| 0),
        ));

        list
    }
}
