use std::fs;
use std::path::PathBuf;

use ron::extensions::Extensions;
use serde::Deserialize;

use crate::error::{Error, Result};
use crate::paths;

/// What the swarm runs on. Only reader of the host file. `trunk` names the branch
/// the project file is read from, so it cannot sit in that file. The rest changes
/// with the machine.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Host {
    /// Project checkout the bare repo fetches from.
    path: String,
    /// Branch of the checkout every task branch starts from.
    pub trunk: String,
    pub workers: usize,
    #[serde(default)]
    pub mounts: Vec<Mount>,
    /// Host environment variables the containers get. Credentials ride here.
    #[serde(default)]
    pub env: Vec<String>,
}

/// One bind: absolute host path, absolute container path. Both sides rw.
#[derive(Debug, Deserialize)]
#[serde(try_from = "String")]
pub struct Mount {
    pub host: String,
    pub guest: String,
}

impl Host {
    pub fn load(project: &str) -> Result<Self> {
        let file = paths::host(project)?;
        let label = paths::arg(&file)?;
        let text =
            fs::read_to_string(&file).map_err(|why| Error::Invalid(format!("{label}: {why}")))?;
        Self::parse(&label, &text)
    }

    pub fn parse(file: &str, text: &str) -> Result<Self> {
        let host: Host = ron::Options::default()
            .with_default_extension(Extensions::IMPLICIT_SOME)
            .from_str(text)
            .map_err(|why| Error::Invalid(format!("{file}: {why}")))?;
        host.check(file)?;
        Ok(host)
    }

    /// Checkout path with a leading `~/` resolved.
    pub fn checkout(&self) -> Result<PathBuf> {
        paths::expand(&self.path).map(PathBuf::from)
    }

    fn check(&self, file: &str) -> Result<()> {
        if !self.path.starts_with('/') && !self.path.starts_with("~/") {
            return Err(invalid(
                file,
                "field `path` must hold a checkout path, absolute or under `~/`",
            ));
        }
        if self.trunk.split_whitespace().count() != 1 || self.trunk.starts_with('-') {
            return Err(invalid(
                file,
                "field `trunk` must name one branch of the checkout",
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
        assert_eq!(host.trunk, "blahblah");
        assert_eq!(host.workers, 3);
        assert_eq!(host.mounts[0].host, "~/alpha/records");
        assert_eq!(host.mounts[0].guest, "/records");
        assert_eq!(host.env, ["ANTHROPIC_API_KEY"]);
    }

    #[test]
    fn names_the_bad_field() {
        let text = EXAMPLE.replace("trunk: \"blahblah\"", "trunk: \"two words\"");
        let why = Host::parse(FILE, &text)
            .expect_err("a trunk of two words must fail")
            .to_string();
        assert!(why.contains("field `trunk`"), "{why}");
    }
}
