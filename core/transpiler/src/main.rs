use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

mod ast;

use clap::Parser;

/// State Machine Language (SML) Compiler
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct Args {
    /// Path to include folder
    #[arg(short('I'), long)]
    include: Vec<String>,

    /// Path to the .sm source file
    filename: String,
}

fn main() -> Result<(), i32> {
    let mut include_path = vec![];
    let mut cargs = Args::parse();
    include_path.push(".".to_owned());
    include_path.append(&mut cargs.include);

    if let Err(e) = ast::ns::Source::compile(
        &cargs.filename,
        include_path,
        Rc::new(RefCell::new(HashSet::new())),
        true,
    ) {
        eprintln!("error: {}:{}", cargs.filename, e);
        std::process::exit(1);
    }
    Ok(())
}
