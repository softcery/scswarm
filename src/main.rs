mod board;
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
mod serve;
mod slot;
mod stage;
mod state;
mod status;
mod turn;

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
    Init { project: String },
    #[command(about = "Run scheduler and workers until stopped")]
    Serve { project: String },
    #[command(about = "Print board rows with state derived from git")]
    Status { project: String },
    #[command(about = "Print the agent transcript of one task")]
    Log {
        project: String,
        id: String,
        #[arg(long, value_name = "N")]
        stage: Option<usize>,
    },
    #[command(about = "Delete the branch and fail mark of one task and of every dependant")]
    Reset { project: String, id: String },
}

fn main() {
    let cli = Cli::parse();
    if let Err(why) = dispatch(cli.command) {
        eprintln!("{why}");
        std::process::exit(why.code());
    }
}

/// Every command names a project: the stem of its host file. The working
/// directory carries no meaning.
fn dispatch(command: Command) -> Result<()> {
    match command {
        Command::Init { project } => init::init(&paths::project(&project)?),
        Command::Serve { project } => {
            let project = paths::project(&project)?;
            serve::serve(&init::open(&project)?, &project)
        }
        Command::Status { project } => {
            let project = paths::project(&project)?;
            status::status(&init::open(&project)?, &project)
        }
        Command::Log { project, id, stage } => log::log(&paths::project(&project)?, &id, stage),
        Command::Reset { project, id } => {
            reset::reset(&init::open(&paths::project(&project)?)?, &id)
        }
    }
}
