use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::credential::{self, Binding};
use crate::device;
use crate::error::{Error, Result};
use crate::git;
use crate::paths::{self, Project};
use crate::yaml;

/// Seconds between scheduler cycles when the host file sets none.
const POLL: u64 = 5;
/// Seconds between fetches of one remote when its entry sets none.
const FETCH: u64 = 60;
/// First rest of a project whose worker failed. A failed remote rests `fetch` first.
pub const REST_FIRST: Duration = Duration::from_secs(60);
/// Longest rest of a project: each rest doubles up to this.
pub const REST_MAX: Duration = Duration::from_secs(900);
/// Interval between stop checks of a running worker.
pub const STOP_POLL: Duration = Duration::from_secs(1);
/// Bounds of each git call to a remote, in seconds and bytes per second: a dead
/// or stalled remote fails the call, so git drops its own locks.
pub const SSH_CONNECT: u64 = 30;
pub const SSH_ALIVE: u64 = 15;
pub const SSH_ALIVE_MISSED: u64 = 4;
pub const LOW_SPEED: u64 = 1000;
pub const LOW_SPEED_TIME: u64 = 60;
/// Glob character that stands for any run of characters in `env` field `match`.
const ANY: char = '*';

/// What the swarm runs on. Only reader of the host file.
#[derive(Debug)]
pub struct Host {
    pub workers: usize,
    /// Host device nodes each container gets at the same path. Resolved at load.
    pub devices: Vec<String>,
    /// Pause between scheduler cycles. Reloads each cycle.
    pub poll: Duration,
    pub projects: BTreeMap<Project, Entry>,
}

/// One project: where its repo text comes from, what its rows get.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Git URL or path the bare repo fetches from and pushes to. `~/` resolved.
    pub remote: String,
    /// Branch name prefix of every track branch.
    pub tracks: String,
    /// Least pause between two fetches of `remote`.
    pub fetch: Duration,
    pub mounts: Vec<Mount>,
    /// Ordered rules: the first rule whose `match` takes a track names its bindings.
    pub env: Vec<EnvRule>,
}

/// Credentials bound into the containers of every track `pattern` matches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvRule {
    pub pattern: String,
    pub set: Vec<Binding>,
}

/// One bind: absolute host path, absolute container path. Both sides rw.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(try_from = "String", into = "String")]
pub struct Mount {
    pub host: String,
    pub guest: String,
}

/// Host file as written. Settles into `Host` once checked.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Written {
    workers: usize,
    #[serde(default)]
    devices: Vec<String>,
    #[serde(default)]
    poll: Option<u64>,
    #[serde(default)]
    projects: BTreeMap<Project, WrittenEntry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WrittenEntry {
    remote: String,
    tracks: String,
    #[serde(default)]
    fetch: Option<u64>,
    #[serde(default)]
    mounts: Vec<Mount>,
    #[serde(default)]
    env: Vec<WrittenRule>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WrittenRule {
    #[serde(rename = "match")]
    pattern: String,
    set: BTreeMap<String, String>,
}

impl Host {
    pub fn load() -> Result<Self> {
        let file = paths::host_file()?;
        let label = paths::arg(&file)?;
        let text =
            fs::read_to_string(&file).map_err(|why| Error::Invalid(format!("{label}: {why}")))?;
        Self::parse(&label, &text)
    }

    pub fn parse(file: &str, text: &str) -> Result<Self> {
        let written: Written = yaml::parse(file, text)?;
        written.check(file)?;
        let host = written.settle()?;
        host.check_mounts(file)?;
        Ok(host)
    }

    /// Entry of `project`. Exit 1 when the host file holds none.
    pub fn entry(&self, project: &Project) -> Result<&Entry> {
        self.projects.get(project).ok_or_else(|| {
            Error::Invalid(format!(
                "project `{project}` must name a key of host file field `projects`"
            ))
        })
    }

    /// No host path bound into two projects, none in the swarm or config dirs: a
    /// shared writable dir lets one project's agent reach another project.
    fn check_mounts(&self, file: &str) -> Result<()> {
        let binds: Vec<(&Project, usize, &Path)> = self
            .projects
            .iter()
            .flat_map(|(project, entry)| {
                entry
                    .mounts
                    .iter()
                    .enumerate()
                    .map(move |(index, mount)| (project, index + 1, Path::new(&mount.host)))
            })
            .collect();
        let overlap = |one: &Path, other: &Path| one.starts_with(other) || other.starts_with(one);
        let reserved = [paths::swarm()?, paths::config()?].map(|dir| paths::real(&dir));
        for (project, entry, path) in &binds {
            if let Some(dir) = reserved.iter().find(|dir| overlap(path, dir)) {
                return Err(invalid(
                    file,
                    &format!(
                        "field `projects.{project}.mounts` entry {entry} must not share a host path with {}",
                        dir.display()
                    ),
                ));
            }
        }
        for (at, (one, one_entry, one_path)) in binds.iter().enumerate() {
            for (other, other_entry, other_path) in &binds[at + 1..] {
                if one != other && overlap(one_path, other_path) {
                    return Err(invalid(
                        file,
                        &format!(
                            "field `projects.{one}.mounts` entry {one_entry} and field `projects.{other}.mounts` entry {other_entry} must not share a host path"
                        ),
                    ));
                }
            }
        }
        Ok(())
    }
}

impl Entry {
    /// Bindings of the first rule that matches `track`. No match: none.
    pub fn env(&self, track: &str) -> &[Binding] {
        self.env
            .iter()
            .find(|rule| matches(&rule.pattern, track))
            .map_or(&[], |rule| &rule.set)
    }
}

impl Written {
    fn check(&self, file: &str) -> Result<()> {
        if self.workers == 0 {
            return Err(invalid(
                file,
                "field `workers` must be a count of 1 or more",
            ));
        }
        if self.poll == Some(0) {
            return Err(invalid(
                file,
                "field `poll` must be seconds, 1 or more, or stay absent",
            ));
        }
        for (index, path) in self.devices.iter().enumerate() {
            if !path.starts_with('/') {
                return Err(invalid(
                    file,
                    &format!(
                        "field `devices` entry {} must hold an absolute host path",
                        index + 1
                    ),
                ));
            }
        }
        for (project, entry) in &self.projects {
            entry
                .check(&format!("projects.{project}"))
                .map_err(|what| invalid(file, &what))?;
        }
        Ok(())
    }

    /// Resolve home paths and device nodes once: later readers hold real paths.
    fn settle(self) -> Result<Host> {
        Ok(Host {
            workers: self.workers,
            devices: device::nodes(&self.devices)?,
            poll: Duration::from_secs(self.poll.unwrap_or(POLL)),
            projects: self
                .projects
                .into_iter()
                .map(|(project, entry)| Ok((project, entry.settle()?)))
                .collect::<Result<_>>()?,
        })
    }
}

impl WrittenEntry {
    /// `field` names the entry, for example `projects.alpha`.
    fn check(&self, field: &str) -> std::result::Result<(), String> {
        if self.remote.trim().is_empty() {
            return Err(format!(
                "field `{field}.remote` must hold a git URL or a repo path"
            ));
        }
        if self.tracks.is_empty() || !git::valid_ref(&format!("{}x", self.tracks)) {
            return Err(format!(
                "field `{field}.tracks` must hold a branch name prefix git takes, not empty: {}",
                git::REF_RULE
            ));
        }
        if self.fetch == Some(0) {
            return Err(format!(
                "field `{field}.fetch` must be seconds, 1 or more, or stay absent"
            ));
        }
        for (index, rule) in self.env.iter().enumerate() {
            let entry = format!("field `{field}.env` entry {}", index + 1);
            if rule.pattern.is_empty() {
                return Err(format!(
                    "{entry}: field `match` must hold a track name glob"
                ));
            }
            if rule
                .set
                .iter()
                .any(|(var, file)| !credential::plain(var) || !credential::plain(file))
            {
                return Err(format!(
                    "{entry}: field `set` must map variable names to credential names: letters, digits, `_`"
                ));
            }
        }
        Ok(())
    }

    fn settle(self) -> Result<Entry> {
        let mounts = self
            .mounts
            .into_iter()
            .map(|mount| {
                let host = paths::expand(&mount.host)?;
                Ok(Mount {
                    host: paths::arg(&paths::real(Path::new(&host)))?,
                    guest: mount.guest,
                })
            })
            .collect::<Result<Vec<Mount>>>()?;
        let env = self
            .env
            .into_iter()
            .map(|rule| EnvRule {
                pattern: rule.pattern,
                set: rule
                    .set
                    .into_iter()
                    .map(|(var, file)| Binding { var, file })
                    .collect(),
            })
            .collect();
        Ok(Entry {
            remote: paths::expand(&self.remote)?,
            tracks: self.tracks,
            fetch: Duration::from_secs(self.fetch.unwrap_or(FETCH)),
            mounts,
            env,
        })
    }
}

impl TryFrom<String> for Mount {
    type Error = String;

    fn try_from(raw: String) -> std::result::Result<Self, String> {
        let parts: Vec<&str> = raw.split(':').collect();
        let [host, guest] = parts[..] else {
            return Err(shaped(&format!("{} parts", parts.len())));
        };
        if !host.starts_with('/') && !host.starts_with("~/") {
            return Err(shaped("a host side neither absolute nor under `~/`"));
        }
        if Path::new(host)
            .components()
            .any(|part| part == Component::ParentDir)
        {
            return Err(shaped("a host side with `..`"));
        }
        if !guest.starts_with('/') {
            return Err(shaped("a container side not absolute"));
        }
        Ok(Self {
            host: host.to_string(),
            guest: guest.to_string(),
        })
    }
}

impl From<Mount> for String {
    fn from(mount: Mount) -> Self {
        format!("{}:{}", mount.host, mount.guest)
    }
}

/// `*` takes any run of characters, every other character takes itself.
fn matches(pattern: &str, name: &str) -> bool {
    let mut parts = pattern.split(ANY);
    let first = parts.next().unwrap_or_default();
    let Some(mut rest) = name.strip_prefix(first) else {
        return false;
    };
    let middle: Vec<&str> = parts.collect();
    let Some((last, middle)) = middle.split_last() else {
        return rest.is_empty();
    };
    for part in middle {
        match rest.find(part) {
            Some(at) => rest = &rest[at + part.len()..],
            None => return false,
        }
    }
    rest.len() >= last.len() && rest.ends_with(last)
}

fn shaped(got: &str) -> String {
    format!("field `mounts` entry must read `/host:/container`: got {got}")
}

fn invalid(file: &str, what: &str) -> Error {
    Error::Invalid(format!("{file}: {what}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "/home/dev/.config/scswarm/host.yaml";

    const EXAMPLE: &str = "\
workers: 3
devices:
  - /dev/null
projects:
  alpha:
    remote: git@github.com:org/alpha.git
    tracks: track/
    fetch: 30
    mounts:
      - ~/alpha/records:/records
    env:
      - match: ai-*
        set: { ANTHROPIC_AUTH_TOKEN: ALPHA_AI_KEY }
      - match: '*'
        set: { ANTHROPIC_AUTH_TOKEN: ALPHA_KEY }
  beta:
    remote: git@github.com:org/beta.git
    tracks: track/
    mounts:
      - /cache/beta:/cache
";

    fn alpha() -> Project {
        "alpha".parse().expect("name must parse")
    }

    #[test]
    fn parses_contract_example() {
        let host = Host::parse(FILE, EXAMPLE).expect("example must parse");
        assert_eq!(host.workers, 3);
        assert_eq!(host.devices, ["/dev/null"]);
        assert_eq!(host.poll, Duration::from_secs(POLL));
        let alpha = host.entry(&alpha()).expect("alpha must stand");
        assert_eq!(alpha.remote, "git@github.com:org/alpha.git");
        assert_eq!(alpha.fetch, Duration::from_secs(30));
        assert_eq!(alpha.mounts[0].guest, "/records");
    }

    #[test]
    fn first_matching_rule_names_the_bindings() {
        let host = Host::parse(FILE, EXAMPLE).expect("example must parse");
        let alpha = host.entry(&alpha()).expect("alpha must stand");
        assert_eq!(alpha.env("ai-search")[0].file, "ALPHA_AI_KEY");
        assert_eq!(alpha.env("net")[0].file, "ALPHA_KEY");
        assert!(matches("a*b*c", "axxbyyc"));
        assert!(!matches("a*b", "ab-c"));
        assert!(!matches("net", "network"));
    }

    #[test]
    fn names_the_bad_field() {
        let text = EXAMPLE.replace("tracks: track/\n    fetch", "tracks: two words/\n    fetch");
        let why = Host::parse(FILE, &text)
            .expect_err("a prefix with a space must fail")
            .to_string();
        assert!(why.contains("field `projects.alpha.tracks`"), "{why}");
    }

    #[test]
    fn refuses_a_host_path_two_projects_share() {
        let text = EXAMPLE.replace("/cache/beta:/cache", "~/alpha:/cache");
        let why = Host::parse(FILE, &text)
            .expect_err("a shared host path must fail")
            .to_string();
        assert!(why.contains("`projects.alpha.mounts` entry 1"), "{why}");
        assert!(why.contains("`projects.beta.mounts` entry 1"), "{why}");
        let text = EXAMPLE.replace("/cache/beta:/cache", "~/swarm/alpha.git:/cache");
        let why = Host::parse(FILE, &text)
            .expect_err("a host path in the swarm dir must fail")
            .to_string();
        assert!(why.contains("`projects.beta.mounts` entry 1"), "{why}");
    }
}
