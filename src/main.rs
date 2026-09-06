mod board;
mod config;
mod error;
mod git;
mod init;
mod log;
mod paths;
mod podman;
mod pool;
mod proc;
mod reset;
mod serve;
mod slot;
mod stage;
mod state;
mod status;
mod turn;

use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::error::Result;

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    #[command(about = "Create bare repo, slot worktrees, image and containers")]
    Init {
        #[arg(long, value_name = "DIR", default_value = ".")]
        root: PathBuf,
    },
    #[command(about = "Run scheduler and workers until stopped")]
    Serve,
    #[command(about = "Print board rows with state derived from git")]
    Status,
    #[command(about = "Print the agent transcript of one task")]
    Log {
        #[arg(value_name = "ID")]
        id: String,
        #[arg(long, value_name = "N")]
        stage: Option<usize>,
    },
    #[command(about = "Delete the branch of one task and of every dependant")]
    Reset {
        #[arg(value_name = "ID")]
        id: String,
    },
}

fn main() {
    let cli = Cli::parse();
    if let Err(why) = dispatch(cli.command) {
        eprintln!("{why}");
        std::process::exit(why.code());
    }
}

fn dispatch(command: Command) -> Result<()> {
    match command {
        Command::Init { root } => init::init(&root),
        Command::Serve => {
            let (project, repo) = init::open()?;
            serve::serve(&repo, &project)
        }
        Command::Status => {
            let (project, repo) = init::open()?;
            status::status(&repo, &project)
        }
        Command::Log { id, stage } => {
            let (project, _) = init::open()?;
            log::log(&project, &id, stage)
        }
        Command::Reset { id } => {
            let (_, repo) = init::open()?;
            reset::reset(&repo, &id)
        }
    }
}
