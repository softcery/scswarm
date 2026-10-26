mod board;
mod call;
mod config;
mod credential;
mod device;
mod error;
mod event;
mod git;
mod host;
mod lock;
mod log;
mod paths;
mod podman;
mod pool;
mod proc;
mod repo;
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
use crate::git::Git;
use crate::host::Host;
use crate::paths::Project;
use crate::state::{Address, Key};
use crate::status::Source;
use crate::work::Job;

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    #[command(
        about = "Run scheduler for every project of the host file until stopped; each row runs as its own process"
    )]
    Serve,
    #[command(name = work::COMMAND, hide = true, about = "Run one row attempt; `serve` spawns it")]
    Work {
        #[command(flatten)]
        job: Job,
    },
    #[command(about = "Print every row of every track with state derived from git")]
    Status {
        /// One project of the host file. Absent: every project.
        project: Option<Project>,
        /// Read the clone in the working directory instead: track branch prefix.
        #[arg(long, value_name = "PREFIX", conflicts_with = "project")]
        tracks: Option<String>,
    },
    #[command(about = "Print the agent transcripts of one row, `<track>/<id>`")]
    Log { project: Project, row: String },
    #[command(
        about = "Delete the branch, land ref and fail refs of one row, `<track>/<id>`, and of every dependant"
    )]
    Reset { project: Project, row: String },
}

fn main() {
    let cli = Cli::parse();
    if let Err(why) = dispatch(cli.command) {
        eprintln!("{why}");
        std::process::exit(why.code());
    }
}

/// Projects are keys of the host file. Working directory means nothing but to
/// `status --tracks`.
fn dispatch(command: Command) -> Result<()> {
    match command {
        Command::Serve => serve::serve(),
        Command::Work { job } => work::work(&repo::open(&job.project)?, &job),
        Command::Status {
            tracks: Some(prefix),
            ..
        } => status::status(&[Source {
            project: None,
            repo: Git::at(paths::here()?),
            tracks: prefix,
        }]),
        Command::Status { project, .. } => {
            let host = Host::load()?;
            let projects = match project {
                Some(project) => vec![project],
                None => host.projects.keys().cloned().collect(),
            };
            let sources = projects
                .into_iter()
                .map(|project| {
                    Ok(Source {
                        tracks: host.entry(&project)?.tracks.clone(),
                        repo: repo::open(&project)?,
                        project: Some(project),
                    })
                })
                .collect::<Result<Vec<Source>>>()?;
            status::status(&sources)
        }
        Command::Log { project, row } => log::log(&Address {
            project,
            key: Key::try_from(row.as_str())?,
        }),
        Command::Reset { project, row } => {
            let tracks = Host::load()?.entry(&project)?.tracks.clone();
            reset::reset(
                &repo::open(&project)?,
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
        let job = Job {
            project: "alpha".parse().expect("project name must pass"),
            slot: 2,
            track: "laser".to_string(),
            id: "rag-04-review".to_string(),
            track_tip: "89abcdef0123456789abcdef0123456789abcdef".to_string(),
            base: "0123456789abcdef0123456789abcdef01234567".to_string(),
            env: vec![credential::Binding {
                var: "ANTHROPIC_AUTH_TOKEN".to_string(),
                file: "ALPHA_KEY".to_string(),
            }],
            mounts: vec![host::Mount {
                host: "/records".to_string(),
                guest: "/mnt".to_string(),
            }],
            devices: vec!["/dev/dri/card0".to_string()],
        };
        let argv = std::iter::once("scswarm".to_string()).chain(job.argv());
        match Cli::try_parse_from(argv).expect("argv must parse").command {
            Command::Work { job: parsed } => assert_eq!(parsed, job),
            _ => panic!("argv must name `work`"),
        }
    }
}
