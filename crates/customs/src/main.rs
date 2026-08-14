mod check;
mod lsp;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "customs", about = "Python import-boundary linter", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Lint Python files for forbidden imports
    Check {
        /// Files or directories to lint (default: current directory)
        #[arg(default_value = ".")]
        paths: Vec<PathBuf>,
    },
    /// Start the Language Server Protocol server on stdio
    Lsp,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Check { paths } => match check::run(&paths) {
            Ok(code) => code,
            Err(err) => {
                eprintln!("error: {err:#}");
                ExitCode::from(2)
            }
        },
        Command::Lsp => {
            if let Err(err) = lsp::run() {
                eprintln!("error: {err:#}");
                ExitCode::from(2)
            } else {
                ExitCode::SUCCESS
            }
        }
    }
}
