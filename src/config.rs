use std::collections::BTreeMap;

use serde::Deserialize;

use crate::error::{Error, Result};
use crate::git::{self, Git};
use crate::paths::Project;
use crate::podman;
use crate::step::{self, Defaults, Steps};
use crate::yaml;

pub const FILE: &str = "scswarm.yaml";

/// Turn text the tool fills in.
pub const ID_SLOT: &str = "{id}";
pub const PARENT_SLOT: &str = "{parent}";
pub const SESSION_SLOT: &str = "{session}";

/// Project settings. Only reader of `scswarm.yaml`.
#[derive(Debug)]
pub struct Config {
    pub image: String,
    pub default_agent: String,
    pub agents: BTreeMap<String, Agent>,
    /// Prompt of every agent commit step: asks for the message the host commits with.
    pub commit: String,
    /// Policy per step kind, under every step field.
    pub defaults: Defaults,
    /// Steps that resolve a conflicted parent merge. Absent: conflict drops branch.
    pub merge: Option<Steps>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Agent {
    pub exec: String,
    #[serde(default)]
    pub resume: Option<String>,
}

/// File as written. Settles into `Config` once checked.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Written {
    image: String,
    default_agent: String,
    agents: BTreeMap<String, Agent>,
    commit: String,
    #[serde(default)]
    defaults: Defaults,
    #[serde(default)]
    merge: Option<Vec<step::Written>>,
}

impl Config {
    pub fn from_trunk(git: &Git) -> Result<Self> {
        Self::at(git, git::TRUNK)
    }

    /// Settings at `rev`, full ref or commit.
    pub fn at(git: &Git, rev: &str) -> Result<Self> {
        let text = git
            .show(rev, FILE)?
            .ok_or_else(|| invalid(&format!("absent at `{rev}`")))?;
        Self::parse(&text)
    }

    pub fn parse(text: &str) -> Result<Self> {
        let written: Written = yaml::parse(FILE, text)?;
        written.check()?;
        written.settle()
    }

    /// Image slots run, in podman's reported form. `image` naming a trunk file:
    /// `init` builds it. Anything else: podman resolves it.
    pub fn image_name(&self, repo: &Git, project: &Project) -> Result<String> {
        Ok(match repo.has_path(git::TRUNK, &self.image)? {
            true => podman::tag(project),
            false => podman::image_ref(&self.image),
        })
    }

    pub fn builds_image(&self, repo: &Git) -> Result<bool> {
        repo.has_path(git::TRUNK, &self.image)
    }

    pub fn knows_agent(&self, key: &str) -> bool {
        self.agents.contains_key(key)
    }

    pub fn agent_keys(&self) -> String {
        keys(&self.agents)
    }
}

impl Written {
    fn check(&self) -> Result<()> {
        if self.image.trim().is_empty() {
            return Err(invalid(
                "field `image` must name a Containerfile path or an image",
            ));
        }
        if self.commit.trim().is_empty() {
            return Err(invalid(
                "field `commit` must hold the prompt that asks for a commit message",
            ));
        }
        self.defaults.check().map_err(|what| invalid(&what))?;
        self.check_agents()
    }

    fn check_agents(&self) -> Result<()> {
        for (key, agent) in &self.agents {
            if agent.exec.trim().is_empty() {
                return Err(invalid(&format!(
                    "agent `{key}` field `exec` must hold the command run in the container"
                )));
            }
            if agent
                .resume
                .as_deref()
                .is_some_and(|it| it.trim().is_empty())
            {
                return Err(invalid(&format!(
                    "agent `{key}` field `resume` must hold arguments or stay absent"
                )));
            }
        }
        if !self.agents.contains_key(&self.default_agent) {
            return Err(invalid(&format!(
                "field `default_agent` must name a key of `agents`: {}",
                keys(&self.agents)
            )));
        }
        Ok(())
    }

    fn settle(self) -> Result<Config> {
        let merge = self
            .merge
            .as_deref()
            .map(|written| step::settle("merge", written, &self.defaults))
            .transpose()
            .map_err(|what| invalid(&what))?;
        Ok(Config {
            image: self.image,
            default_agent: self.default_agent,
            agents: self.agents,
            commit: self.commit,
            defaults: self.defaults,
            merge,
        })
    }
}

fn keys(agents: &BTreeMap<String, Agent>) -> String {
    agents.keys().cloned().collect::<Vec<_>>().join(", ")
}

fn invalid(what: &str) -> Error {
    Error::Invalid(format!("{FILE}: {what}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = r#"
image: Containerfile.swarm
default_agent: claude
agents:
  claude:
    exec: claude --model opus -p --output-format json
    resume: --resume {session}
commit: "Reply with the commit message for the work in this session. Message only."
defaults:
  prompt: { tries: 3, timeout: 3600 }
  run: { tries: 1, timeout: 7200 }
merge:
  - "Resolve every conflict of the merge of {parent} into {id}."
  - run: make lint
  - commit:
"#;

    #[test]
    fn parses_contract_example() {
        let config = Config::parse(EXAMPLE).expect("example must parse");
        assert_eq!(config.image, "Containerfile.swarm");
        assert!(config.commit.starts_with("Reply with"), "{}", config.commit);
        assert_eq!(config.merge.expect("merge must parse").len(), 3);
        assert_eq!(
            config.agents["claude"].resume.as_deref(),
            Some("--resume {session}")
        );
    }

    #[test]
    fn names_the_bad_field() {
        let text = EXAMPLE.replace("default_agent: claude", "default_agent: codex");
        let why = Config::parse(&text)
            .expect_err("unknown agent must fail")
            .to_string();
        assert!(why.contains("field `default_agent`"), "{why}");
    }
}
