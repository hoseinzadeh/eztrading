//! `ezt-agent`: write, repair and explain SML trading strategies with an LLM
//! that is held to the real compiler.

use clap::{Args, Parser, Subcommand};
use ezt_agent::agent::{self, Config, Context, Task};
use ezt_agent::compiler::{Checker, Validator};
use ezt_agent::llm;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[derive(Parser)]
#[command(name = "ezt-agent", version, about, long_about = None)]
struct Cli {
    /// Path to the SML compiler (`compile`). Defaults to the one next to this binary, then PATH.
    #[arg(long, global = true, env = "EZT_COMPILER")]
    compiler: Option<PathBuf>,

    /// Directory with the SML standard library
    #[arg(long, global = true, env = "EZT_STDLIB", default_value = "package/libs")]
    stdlib: PathBuf,

    #[command(subcommand)]
    command: Command,
}

#[derive(Args)]
struct ModelOpts {
    /// Claude model to use
    #[arg(long, env = "EZT_AGENT_MODEL", default_value = "claude-opus-5-5")]
    model: String,

    /// Reasoning effort: low, medium, high, xhigh or max (omit for the model default)
    #[arg(long, env = "EZT_AGENT_EFFORT")]
    effort: Option<String>,

    /// Maximum model turns before giving up
    #[arg(long, default_value_t = 12)]
    max_turns: usize,

    /// Allow inline C++ ({% %}) in the result. Such strategies run as native code: review them.
    #[arg(long)]
    allow_cpp: bool,

    /// Show each tool call as it happens
    #[arg(short, long)]
    verbose: bool,
}

#[derive(Subcommand)]
enum Command {
    /// Write a new strategy from a plain-language description
    Generate {
        /// What the strategy should do, e.g. "buy AAPL after two down days, sell at +10%"
        description: String,
        /// Where to write the strategy
        #[arg(short, long, default_value = "strategy.sm")]
        output: PathBuf,
        /// Overwrite the output file if it exists
        #[arg(long)]
        force: bool,
        #[command(flatten)]
        model: ModelOpts,
    },
    /// Repair a strategy that does not compile
    Fix {
        file: PathBuf,
        /// Write the result here instead of <file>.fixed.sm
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Overwrite the input file
        #[arg(long, conflicts_with = "output")]
        in_place: bool,
        #[command(flatten)]
        model: ModelOpts,
    },
    /// Explain what a strategy does and where it is risky (read-only)
    Explain {
        file: PathBuf,
        #[command(flatten)]
        model: ModelOpts,
    },
    /// Compile a strategy and run the lints (no API key needed)
    Check { file: PathBuf },
}

fn main() -> ExitCode {
    match real_main() {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(1)
        }
    }
}

fn real_main() -> Result<ExitCode, String> {
    let cli = Cli::parse();
    let compiler = Checker::locate_compiler(cli.compiler.as_deref());
    let checker = Checker::new(compiler, cli.stdlib.clone());

    match cli.command {
        Command::Check { file } => {
            let report = checker.check(&read(&file)?);
            println!("{}", report.render());
            Ok(if report.ok { ExitCode::SUCCESS } else { ExitCode::from(1) })
        }
        Command::Generate { description, output, force, model } => {
            guard_output(&output, force)?;
            let outcome = run_agent(&cli.stdlib, &checker, &model, Task::Generate(&description))?;
            finish(outcome, Some(&output))
        }
        Command::Fix { file, output, in_place, model } => {
            let source = read(&file)?;
            let first = checker.check(&source);
            if first.ok {
                println!("{} already passes all checks; nothing to fix.", file.display());
                return Ok(ExitCode::SUCCESS);
            }
            let target = if in_place {
                file.clone()
            } else {
                output.unwrap_or_else(|| file.with_extension("fixed.sm"))
            };
            guard_output(&target, in_place)?;
            let diagnostics = first.render();
            let outcome = run_agent(&cli.stdlib, &checker, &model, Task::Fix { source: &source, diagnostics: &diagnostics })?;
            finish(outcome, Some(&target))
        }
        Command::Explain { file, model } => {
            let outcome = run_agent(&cli.stdlib, &checker, &model, Task::Explain(&read(&file)?))?;
            println!("{}", outcome.summary);
            report_usage(&outcome);
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn run_agent(stdlib: &Path, checker: &Checker, opts: &ModelOpts, task: Task) -> Result<agent::Outcome, String> {
    let std_sm = read(&stdlib.join("std.sm"))?;
    let llm = llm::Anthropic::from_env(&opts.model, opts.effort.clone())?;
    let cfg = Config { max_turns: opts.max_turns, allow_cpp: opts.allow_cpp, verbose: opts.verbose };
    let ctx = Context { llm: &llm, validator: checker, stdlib: &std_sm };
    agent::run(&ctx, task, &cfg)
}

fn finish(outcome: agent::Outcome, output: Option<&Path>) -> Result<ExitCode, String> {
    match (&outcome.source, output) {
        (Some(source), Some(path)) => {
            std::fs::write(path, source).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
            println!("Wrote {} (passed the compiler and lints).\n\n{}", path.display(), outcome.summary);
            println!("\nReview the strategy before running it; paper-trade first. This is not financial advice.");
            report_usage(&outcome);
            Ok(ExitCode::SUCCESS)
        }
        _ => {
            println!("No strategy was produced.\n\n{}", outcome.summary);
            report_usage(&outcome);
            Ok(ExitCode::from(2))
        }
    }
}

fn report_usage(o: &agent::Outcome) {
    eprintln!(
        "[{} turns, {} compiler checks, {} input / {} output tokens]",
        o.turns, o.checks, o.input_tokens, o.output_tokens
    );
}

fn read(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))
}

fn guard_output(path: &Path, allow_overwrite: bool) -> Result<(), String> {
    if path.exists() && !allow_overwrite {
        return Err(format!("{} already exists (use --force to overwrite)", path.display()));
    }
    Ok(())
}
