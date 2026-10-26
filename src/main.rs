mod board;
mod call;
mod config;
mod credential;
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

use crate::error::{Error, Result};
use crate::git::Git;
use crate::host::Host;
use crate::paths::Project;
use crate::state::Key;
use crate::work::Job;

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    #[command(about = "Create bare repo and `origin`, fetch")]
    Init { project: String },
    #[command(about = "Run scheduler until stopped; each row runs as its own process")]
    Serve { project: String },
    #[command(name = work::COMMAND, hide = true, about = "Run one row attempt; `serve` spawns it")]
    Work {
        project: String,
        #[command(flatten)]
        job: Job,
    },
    #[command(about = "Print every row of every track with state derived from git")]
    Status {
        #[arg(required_unless_present = "tracks")]
        project: Option<String>,
        /// Read the clone in the working directory instead: track branch prefix.
        #[arg(long, value_name = "PREFIX", conflicts_with = "project")]
        tracks: Option<String>,
    },
    #[command(about = "Print the agent transcripts of one row, `<track>/<id>`")]
    Log { project: String, row: String },
    #[command(
        about = "Delete the branch, land ref and fail refs of one row, `<track>/<id>`, and of every dependant"
    )]
    Reset { project: String, row: String },
}

fn main() {
    let cli = Cli::parse();
    if let Err(why) = dispatch(cli.command) {
        eprintln!("{why}");
        std::process::exit(why.code());
    }
}

/// Every command but `status --tracks` names a project: stem of its host file.
/// Working directory means nothing there.
fn dispatch(command: Command) -> Result<()> {
    let named = |name: &str| Project::try_from(name);
    match command {
        Command::Init { project } => init::init(&named(&project)?),
        Command::Serve { project } => {
            let project = named(&project)?;
            serve::serve(&init::open(&project)?, &project)
        }
        Command::Work { project, job } => {
            let project = named(&project)?;
            work::work(&init::open(&project)?, &project, &job)
        }
        Command::Status {
            project: Some(project),
            ..
        } => {
            let project = named(&project)?;
            let tracks = Host::load(&project)?.tracks;
            status::status(&init::open(&project)?, &tracks, Some(&project))
        }
        Command::Status {
            tracks: Some(prefix),
            ..
        } => status::status(&Git::at(paths::here()?), &prefix, None),
        Command::Status { .. } => Err(Error::Invalid(
            "`status` must name a project, or a track branch prefix in `--tracks`".to_string(),
        )),
        Command::Log { project, row } => log::log(&named(&project)?, &Key::try_from(row.as_str())?),
        Command::Reset { project, row } => {
            let project = named(&project)?;
            let tracks = Host::load(&project)?.tracks;
            reset::reset(
                &init::open(&project)?,
                &tracks,
                &Key::try_from(row.as_str())?,
            )
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
            track: "laser".to_string(),
            id: "rag-04-review".to_string(),
            track_tip: "89abcdef0123456789abcdef0123456789abcdef".to_string(),
            base: "0123456789abcdef0123456789abcdef01234567".to_string(),
            env: vec!["ANTHROPIC_API_KEY".to_string()],
            mounts: vec![host::Mount {
                host: "/records".to_string(),
                guest: "/mnt".to_string(),
            }],
            devices: vec!["/dev/dri/card0".to_string()],
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
