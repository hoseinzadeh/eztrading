use ci::{JobSrvCI, CommandInterface};
use clap::Parser;
use com::status::Status;
use rustyline::error::ReadlineError;
use rustyline::history::DefaultHistory;
use rustyline::{Editor, Result};
mod ci;
mod terminal;
use terminal::Terminal;

/// A Command-Line Interface to manage servers
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// The IP address of the server (e.g., "10.1.1.5:2020")
    #[arg(short, long)]
    sever_ip: Option<String>,
    //    /// Database connection (format: "ip:port,dbname,user,pass")
    //    #[arg(short, long)]
    //    database: String,
}

fn main() -> Result<()> {
    let args = Args::parse();

    let home = home::home_dir().expect("Could not find the home directory");
    let home = home.to_str().unwrap();
    let dir = format!("{home}/.eztrading");
    let hist_file = format!("{dir}/cli-history");
    std::fs::create_dir_all(&dir).unwrap();

    let mut rl = Editor::<(), DefaultHistory>::new()?;
    if let Err(e) = rl.load_history(hist_file.as_str()) {
        println!("No previous history: {e}");
    }

    let term = Terminal::new();
    
    term.intake_commands(JobSrvCI::default_commands()).unwrap();

    if let Some(server_ip) = args.sever_ip {
        match terminal::execute(format!("connect {}", server_ip).as_str(), &term) {
            Ok(_) => (),
            Err(Status::Failed(e)) => errprintln!("{}", e.what()),
            Err(_) => ()
        }
    }

    loop {
        let readline = rl.readline(&term.prompt.to_string());
        match readline {
            Ok(line) => {
                let _=rl.add_history_entry(line.as_str());
                match terminal::execute(&line, &term) {
                    Ok(s) => {
                        if s == -1 {
                            break;
                        }
                    }
                    Err(Status::Failed(e)) => errprintln!("{}", e.what()),
                    Err(_) => ()
                }
            }
            Err(ReadlineError::Interrupted) => {
                // use std::io::{Write, stdout};
                // if buf.is_empty() {
                //     if stdout().write("Exit CLI? [y/n]: ".as_bytes()).is_ok() {
                //         if stdout().flush().is_ok() {
                //             let mut ch = String::new();
                //             if std::io::stdin().read_line(&mut ch).is_ok() {
                //                 if ch.trim().to_lowercase() == "y" {
                //                     break;
                //                 }
                //             }
                //             continue;
                //         }
                //         break;
                //     }
                //     break;
                // } else {
                //     continue;
                // }
                continue;
            }
            Err(ReadlineError::Eof) => {
                if term.connected() {
                    let _=term.disconnect();
                } else {
                    break;
                }
            }
            Err(err) => {
                errprintln!("Error: {:?}", err);
                break;
            }
        }
    }
    term.prompt.0.write().unwrap().clear();
    rl.save_history(hist_file.as_str())
}