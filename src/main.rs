mod board;
mod call;
mod config;
mod error;
mod event;
mod git;
mod host;
mod init;
mod log;
mod paths;
mod podman;
mod pool;
mod proc;
mod reset;
mod script;
mod serve;
mod slot;
mod state;
mod status;
mod step;
mod yaml;

use clap::{Parser, Subcommand};

use crate::error::Result;
use crate::paths::Project;

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    #[command(about = "Create bare repo, slot worktrees, image and containers")]
    Init { project: String },
    #[command(about = "Run scheduler and workers until stopped")]
    Serve { project: String },
    #[command(about = "Print board rows with state derived from git")]
    Status { project: String },
    #[command(about = "Print the agent transcript of one task")]
    Log { project: String, id: String },
    #[command(
        about = "Delete the branch, land ref and fail mark of one task and of every dependant"
    )]
    Reset { project: String, id: String },
}

fn main() {
    let cli = Cli::parse();
    if let Err(why) = dispatch(cli.command) {
        eprintln!("{why}");
        std::process::exit(why.code());
    }
}

/// Every command names a project: stem of its host file. Working directory means
/// nothing.
fn dispatch(command: Command) -> Result<()> {
    let project = Project::try_from(command.project())?;
    match command {
        Command::Init { .. } => init::init(&project),
        Command::Serve { .. } => serve::serve(&init::open(&project)?, &project),
        Command::Status { .. } => status::status(&init::open(&project)?, &project),
        Command::Log { id, .. } => log::log(&project, &id),
        Command::Reset { id, .. } => reset::reset(&init::open(&project)?, &id),
    }
}

impl Command {
    fn project(&self) -> &str {
        match self {
            Command::Init { project }
            | Command::Serve { project }
            | Command::Status { project }
            | Command::Log { project, .. }
            | Command::Reset { project, .. } => project,
        }
    }
}
