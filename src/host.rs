use std::fs;
use std::path::PathBuf;

use ron::extensions::Extensions;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::git;
use crate::paths::{self, Project};

/// What the swarm runs on. Only reader of the host file.
#[derive(Debug)]
pub struct Host {
    pub remote: Remote,
    pub slots: Slots,
}

/// Trunk source, reloaded each cycle. Fields become `remote.origin.url` and
/// `remote.origin.fetch`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remote {
    /// Project checkout the bare repo fetches from. Repo root, `~/` resolved.
    pub path: PathBuf,
    /// Branch of the checkout every task branch starts from.
    pub trunk: String,
}

/// What `init` bakes into the slot set. Immutable while `serve` runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slots {
    pub workers: usize,
    pub mounts: Vec<Mount>,
    /// Host environment variables the containers get. Credentials ride here.
    pub env: Vec<String>,
}

/// Slot set as text. A container carries it, `serve` compares it back.
#[derive(Debug, PartialEq, Eq)]
pub struct Stamp {
    pub image: String,
    pub mounts: String,
    pub env: String,
}

/// One bind: absolute host path, absolute container path. Both sides rw.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(try_from = "String", into = "String")]
pub struct Mount {
    pub host: String,
    pub guest: String,
}

/// Host file as written. Splits into `Remote` and `Slots` once checked.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Written {
    path: String,
    trunk: String,
    workers: usize,
    #[serde(default)]
    mounts: Vec<Mount>,
    #[serde(default)]
    env: Vec<String>,
}

impl Host {
    pub fn load(project: &Project) -> Result<Self> {
        let file = paths::host_file(project)?;
        let label = paths::arg(&file)?;
        let text =
            fs::read_to_string(&file).map_err(|why| Error::Invalid(format!("{label}: {why}")))?;
        Self::parse(&label, &text)
    }

    pub fn parse(file: &str, text: &str) -> Result<Self> {
        let written: Written = ron::Options::default()
            .with_default_extension(Extensions::IMPLICIT_SOME)
            .from_str(text)
            .map_err(|why| Error::Invalid(format!("{file}: {why}")))?;
        written.check(file)?;
        written.split()
    }
}

impl Slots {
    /// Text the containers carry: `image` plus the two fields they bake.
    pub fn stamp(&self, image: &str) -> Result<Stamp> {
        Ok(Stamp {
            image: image.to_string(),
            mounts: text(&self.mounts)?,
            env: text(&self.env)?,
        })
    }

    /// First field that differs. `None` when the two sets match.
    pub fn drift(&self, other: &Self) -> Option<&'static str> {
        match (self.workers == other.workers, self.mounts == other.mounts) {
            (false, _) => Some("field `workers`"),
            (_, false) => Some("field `mounts`"),
            _ => (self.env != other.env).then_some("field `env`"),
        }
    }
}

impl Stamp {
    /// First field that differs. `None` when the container carries this stamp.
    pub fn drift(&self, other: &Self) -> Option<&'static str> {
        match (self.image == other.image, self.mounts == other.mounts) {
            (false, _) => Some("field `image`"),
            (_, false) => Some("field `mounts`"),
            _ => (self.env != other.env).then_some("field `env`"),
        }
    }
}

impl Written {
    fn check(&self, file: &str) -> Result<()> {
        if !self.path.starts_with('/') && !self.path.starts_with("~/") {
            return Err(invalid(
                file,
                "field `path` must hold the project repo root, absolute or under `~/`",
            ));
        }
        if !git::valid_ref(&self.trunk) {
            return Err(invalid(
                file,
                &format!(
                    "field `trunk` must name a branch git takes: {}",
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
        self.check_env(file)
    }

    fn check_env(&self, file: &str) -> Result<()> {
        for (index, name) in self.env.iter().enumerate() {
            let plain = |letter: char| letter.is_ascii_alphanumeric() || letter == '_';
            if name.is_empty() || !name.chars().all(plain) {
                return Err(invalid(
                    file,
                    &format!(
                        "field `env` entry {} must name a host environment variable",
                        index + 1
                    ),
                ));
            }
        }
        Ok(())
    }

    /// Resolve home paths once: later readers hold real paths.
    fn split(self) -> Result<Host> {
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
            remote: Remote {
                path: PathBuf::from(paths::expand(&self.path)?),
                trunk: self.trunk,
            },
            slots: Slots {
                workers: self.workers,
                mounts,
                env: self.env,
            },
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

fn text<T: Serialize>(value: &T) -> Result<String> {
    ron::to_string(value).map_err(|why| Error::Failed(format!("slot set must serialise: {why}")))
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

    const FILE: &str = "/home/dev/.config/scswarm/alpha.ron";

    const EXAMPLE: &str = r#"
(
  path: "~/alpha",
  trunk: "blahblah",
  workers: 3,
  mounts: ["~/alpha/records:/records"],
  env: ["ANTHROPIC_API_KEY"],
)
"#;

    #[test]
    fn parses_contract_example() {
        let host = Host::parse(FILE, EXAMPLE).expect("example must parse");
        assert!(
            host.remote.path.ends_with("alpha"),
            "{:?}",
            host.remote.path
        );
        assert_eq!(host.remote.trunk, "blahblah");
        assert_eq!(host.slots.workers, 3);
        assert_eq!(host.slots.mounts[0].guest, "/records");
        assert_eq!(host.slots.env, ["ANTHROPIC_API_KEY"]);
    }

    #[test]
    fn names_the_bad_field() {
        let text = EXAMPLE.replace("trunk: \"blahblah\"", "trunk: \"two words\"");
        let why = Host::parse(FILE, &text)
            .expect_err("a trunk of two words must fail")
            .to_string();
        assert!(why.contains("field `trunk`"), "{why}");
    }

    #[test]
    fn stamps_mounts_and_env() {
        let host = Host::parse(FILE, EXAMPLE).expect("example must parse");
        let stamp = host
            .slots
            .stamp("localhost/scswarm/alpha")
            .expect("stamp");
        assert!(
            stamp.mounts.ends_with("alpha/records:/records\"]"),
            "{}",
            stamp.mounts
        );
        assert_eq!(stamp.env, "[\"ANTHROPIC_API_KEY\"]");
    }
}
