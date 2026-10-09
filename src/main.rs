mod board;
mod call;
mod config;
mod device;
mod error;
mod event;
mod git;
mod host;
mod init;
mod lock;
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
mod transcript;
mod work;
mod yaml;

use clap::{Parser, Subcommand};

use crate::error::Result;
use crate::paths::Project;
use crate::work::Job;

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    #[command(about = "Create bare repo and image")]
    Init { project: String },
    #[command(about = "Run scheduler until stopped; each row runs as its own process")]
    Serve { project: String },
    #[command(name = work::COMMAND, hide = true, about = "Run one row attempt; `serve` spawns it")]
    Work {
        project: String,
        #[command(flatten)]
        job: Job,
    },
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
        Command::Work { job, .. } => work::work(&init::open(&project)?, &project, &job),
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
            | Command::Work { project, .. }
            | Command::Status { project }
            | Command::Log { project, .. }
            | Command::Reset { project, .. } => project,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Argv is the contract between an old `serve` and a new worker binary.
    #[test]
    fn job_argv_parses_back() {
        let project = Project::try_from("alpha").expect("project name must pass");
        let job = Job {
            slot: 2,
            id: "rag-04-review".to_string(),
            base: "0123456789abcdef0123456789abcdef01234567".to_string(),
            stack: vec![
                "swarm/rag-04-retriever".to_string(),
                "swarm/rag-03".to_string(),
            ],
            env: vec!["ANTHROPIC_API_KEY".to_string()],
        };
        let argv = std::iter::once("scswarm".to_string()).chain(job.argv(&project));
        match Cli::try_parse_from(argv).expect("argv must parse").command {
            Command::Work {
                project: name,
                job: parsed,
            } => {
                assert_eq!(name, "alpha");
                assert_eq!(parsed, job);
            }
            _ => panic!("argv must name `work`"),
        }
    }
}
