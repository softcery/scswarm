mod board;
mod call;
mod config;
mod credential;
mod device;
mod error;
mod event;
mod feed;
mod git;
mod help;
mod host;
mod lock;
mod log;
mod man;
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

use std::io;
use std::path::PathBuf;

use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};
use clap_complete::Shell;

use crate::error::{Error, Result};
use crate::git::Git;
use crate::host::Host;
use crate::paths::Project;
use crate::state::{Address, Key};
use crate::status::Source;
use crate::work::Job;

#[derive(Parser)]
#[command(
    version = help::VERSION,
    about,
    long_about = help::ROOT_ABOUT,
    after_help = help::ROOT_EXAMPLES
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    #[command(
        about = "Run the scheduler for every project of the host file until stopped",
        long_about = help::SERVE_ABOUT
    )]
    Serve,
    #[command(name = work::COMMAND, hide = true, about = "Run one row attempt; `serve` spawns it")]
    Work {
        #[command(flatten)]
        job: Job,
    },
    #[command(
        about = "Print the state of every row of every track",
        long_about = help::STATUS_ABOUT
    )]
    Status {
        /// Key of `projects` in the host file. Absent: every project.
        project: Option<Project>,
        /// Track branch prefix, for example `track/`. Reads the clone in this
        /// directory instead of the host.
        #[arg(long, value_name = "PREFIX", conflicts_with = "project")]
        tracks: Option<String>,
    },
    #[command(
        about = "Print the agent transcripts of one row",
        long_about = help::LOG_ABOUT
    )]
    Log {
        /// Key of `projects` in the host file.
        project: Project,
        /// Row as `<track>/<id>`, for example `laser/retriever`.
        row: String,
    },
    #[command(
        about = "Delete one row and its dependants so they run again",
        long_about = help::RESET_ABOUT
    )]
    Reset {
        /// Key of `projects` in the host file.
        project: Project,
        /// Row as `<track>/<id>`, for example `laser/retriever`.
        row: String,
    },
    #[command(about = "Print the shell completion script for SHELL")]
    Completions { shell: Shell },
    #[command(about = "Write one man page per command into DIR")]
    Man {
        /// Directory that exists, for example `~/.local/share/man/man1`.
        dir: PathBuf,
    },
}

fn main() {
    let parsed = cli()
        .try_get_matches()
        .and_then(|matches| Cli::from_arg_matches(&matches));
    let cli = parsed.unwrap_or_else(|why| {
        let _ = why.print();
        std::process::exit(usage_code(&why));
    });
    if let Err(why) = dispatch(cli.command) {
        eprintln!("{why}");
        std::process::exit(why.code());
    }
}

/// Help, version: 0. User usage error: 1. Bad `work` argv means binary
/// mismatch with `serve`: 2, so its project rests as on machine fault.
fn usage_code(why: &clap::Error) -> i32 {
    match (why.use_stderr(), std::env::args().nth(1).as_deref()) {
        (false, _) => 0,
        (true, Some(work::COMMAND)) => error::FAILED,
        (true, _) => error::INVALID,
    }
}

/// Command tree with every help section attached: parse and man read this one.
fn cli() -> clap::Command {
    help::attach(Cli::command())
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
        } => status::status(
            &[Source {
                project: None,
                repo: Git::at(paths::here()?),
                tracks: prefix,
            }],
            Vec::new(),
        ),
        Command::Status { project, .. } => {
            let host = Host::load()?;
            let projects = match project {
                Some(project) => vec![project],
                None => host.projects.keys().cloned().collect(),
            };
            let (mut sources, mut faults) = (Vec::new(), Vec::new());
            for project in projects {
                let tracks = host.entry(&project)?.tracks.clone();
                match repo::open(&project) {
                    Ok(repo) => sources.push(Source {
                        project: Some(project),
                        repo,
                        tracks,
                    }),
                    Err(why) => faults.push(why),
                }
            }
            status::status(&sources, faults)
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
        Command::Completions { shell } => {
            clap_complete::generate(shell, &mut cli(), help::ROOT_NAME, &mut io::stdout());
            Ok(())
        }
        Command::Man { dir } => man::write(cli(), &dir)
            .map_err(|why| Error::Failed(format!("man pages into {}: {why}", dir.display()))),
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
