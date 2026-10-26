use std::fs;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::credential;
use crate::device;
use crate::error::{Error, Result};
use crate::git;
use crate::paths::{self, Project};
use crate::yaml;

/// Seconds between loop cycles when the host file sets none.
const POLL: u64 = 5;
/// Glob character that stands for any run of characters in `env` field `match`.
const ANY: char = '*';

/// What the swarm runs on. Only reader of the host file.
#[derive(Debug)]
pub struct Host {
    /// Git URL or path the bare repo fetches from and pushes to. `~/` resolved.
    pub remote: String,
    /// Branch name prefix of every track branch.
    pub tracks: String,
    pub workers: usize,
    pub mounts: Vec<Mount>,
    /// Host device nodes each container gets at the same path. Resolved at load.
    pub devices: Vec<String>,
    /// Ordered rules: the first rule whose `match` takes a track names its variables.
    pub env: Vec<EnvRule>,
    /// Pause between loop cycles. Reloads each cycle.
    pub poll: Duration,
}

/// Credentials forwarded into the containers of every track `pattern` matches.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvRule {
    #[serde(rename = "match")]
    pub pattern: String,
    pub names: Vec<String>,
}

/// What a slot container bakes, as text. A container carries it, the worker
/// compares it back.
#[derive(Debug, PartialEq, Eq)]
pub struct Stamp {
    /// Image ID: a rebuild under the same name reads as a change.
    pub image: String,
    pub mounts: String,
    pub devices: String,
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
    remote: String,
    tracks: String,
    workers: usize,
    #[serde(default)]
    mounts: Vec<Mount>,
    #[serde(default)]
    devices: Vec<String>,
    #[serde(default)]
    env: Vec<EnvRule>,
    #[serde(default)]
    poll: Option<u64>,
}

impl Host {
    pub fn load(project: &Project) -> Result<Self> {
        let file = paths::host_file(project)?;
        let label = paths::arg(&file)?;
        let text =
            fs::read_to_string(&file).map_err(|why| Error::Invalid(format!("{label}: {why}")))?;
        let host = Self::parse(&label, &text)?;
        let names: Vec<String> = host
            .env
            .iter()
            .flat_map(|rule| rule.names.iter().cloned())
            .collect();
        credential::check(&names)?;
        Ok(host)
    }

    pub fn parse(file: &str, text: &str) -> Result<Self> {
        let written: Written = yaml::parse(file, text)?;
        written.check(file)?;
        written.settle()
    }

    /// Variables of the first rule that matches `track`. No match: none.
    pub fn env(&self, track: &str) -> &[String] {
        self.env
            .iter()
            .find(|rule| matches(&rule.pattern, track))
            .map_or(&[], |rule| &rule.names)
    }
}

impl Stamp {
    /// Text a container carries: image ID plus the mounts and devices it bakes.
    pub fn new(image: &str, mounts: &[Mount], devices: &[String]) -> Result<Self> {
        Ok(Self {
            image: image.to_string(),
            mounts: text(&mounts)?,
            devices: text(&devices)?,
        })
    }
}

impl Written {
    fn check(&self, file: &str) -> Result<()> {
        if self.remote.trim().is_empty() {
            return Err(invalid(
                file,
                "field `remote` must hold a git URL or a repo path",
            ));
        }
        if self.tracks.is_empty() || !git::valid_ref(&format!("{}x", self.tracks)) {
            return Err(invalid(
                file,
                &format!(
                    "field `tracks` must hold a branch name prefix git takes, not empty: {}",
                    git::REF_RULE
                ),
            ));
        }
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
        self.check_devices(file)?;
        self.check_env(file)
    }

    fn check_devices(&self, file: &str) -> Result<()> {
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
        Ok(())
    }

    fn check_env(&self, file: &str) -> Result<()> {
        let plain = |letter: char| letter.is_ascii_alphanumeric() || letter == '_';
        for (index, rule) in self.env.iter().enumerate() {
            let entry = format!("field `env` entry {}", index + 1);
            if rule.pattern.is_empty() {
                return Err(invalid(
                    file,
                    &format!("{entry}: field `match` must hold a track name glob"),
                ));
            }
            if rule
                .names
                .iter()
                .any(|name| name.is_empty() || !name.chars().all(plain))
            {
                return Err(invalid(
                    file,
                    &format!(
                        "{entry}: field `names` must list credential names: letters, digits, `_`"
                    ),
                ));
            }
        }
        Ok(())
    }

    /// Resolve home paths and device nodes once: later readers hold real paths.
    fn settle(self) -> Result<Host> {
        let mounts = self
            .mounts
            .into_iter()
            .map(|mount| {
                Ok(Mount {
                    host: paths::expand(&mount.host)?,
                    guest: mount.guest,
                })
            })
            .collect::<Result<Vec<Mount>>>()?;
        Ok(Host {
            remote: paths::expand(&self.remote)?,
            tracks: self.tracks,
            workers: self.workers,
            mounts,
            devices: device::nodes(&self.devices)?,
            env: self.env,
            poll: Duration::from_secs(self.poll.unwrap_or(POLL)),
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

/// Stamp text a container carries as a label. JSON: the YAML reader only reads.
fn text<T: Serialize>(value: &T) -> Result<String> {
    serde_json::to_string(value)
        .map_err(|why| Error::Failed(format!("slot binds must serialise: {why}")))
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

    const FILE: &str = "/home/dev/.config/scswarm/alpha.yaml";

    const EXAMPLE: &str = "\
remote: git@github.com:org/alpha.git
tracks: track/
workers: 3
mounts:
  - ~/alpha/records:/records
devices:
  - /dev/null
env:
  - match: ai-*
    names: [AI_TOKEN]
  - match: '*'
    names: [ANTHROPIC_API_KEY]
";

    #[test]
    fn parses_contract_example() {
        let host = Host::parse(FILE, EXAMPLE).expect("example must parse");
        assert_eq!(host.remote, "git@github.com:org/alpha.git");
        assert_eq!(host.tracks, "track/");
        assert_eq!(host.workers, 3);
        assert_eq!(host.mounts[0].guest, "/records");
        assert_eq!(host.devices, ["/dev/null"]);
    }

    #[test]
    fn first_matching_rule_names_the_variables() {
        let host = Host::parse(FILE, EXAMPLE).expect("example must parse");
        assert_eq!(host.env("ai-search"), ["AI_TOKEN"]);
        assert_eq!(host.env("net"), ["ANTHROPIC_API_KEY"]);
        assert!(matches("a*b*c", "axxbyyc"));
        assert!(!matches("a*b", "ab-c"));
        assert!(!matches("net", "network"));
    }

    #[test]
    fn names_the_bad_field() {
        let text = EXAMPLE.replace("tracks: track/", "tracks: two words/");
        let why = Host::parse(FILE, &text)
            .expect_err("a prefix with a space must fail")
            .to_string();
        assert!(why.contains("field `tracks`"), "{why}");
    }
}
