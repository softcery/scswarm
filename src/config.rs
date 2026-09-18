use std::collections::BTreeMap;

use serde::Deserialize;

use crate::error::{Error, Result};
use crate::git::{self, Git};
use crate::paths::Project;
use crate::podman;
use crate::yaml;

pub const FILE: &str = "scswarm.yaml";

/// Turn text the tool fills in.
pub const ID_SLOT: &str = "{id}";
pub const PARENT_SLOT: &str = "{parent}";
pub const SESSION_SLOT: &str = "{session}";

/// Project settings. Only reader of `scswarm.yaml`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub image: String,
    pub default_agent: String,
    pub agents: BTreeMap<String, Agent>,
    /// Last turn of every session: asks for the message the host commits with.
    pub commit: String,
    /// Turns that resolve a conflicted parent merge. Absent: conflict drops branch.
    #[serde(default)]
    pub merge: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Agent {
    pub exec: String,
    #[serde(default)]
    pub resume: Option<String>,
}

impl Config {
    pub fn from_trunk(git: &Git) -> Result<Self> {
        let text = git
            .show(git::TRUNK, FILE)?
            .ok_or_else(|| invalid("absent from the trunk branch"))?;
        Self::parse(&text)
    }

    pub fn parse(text: &str) -> Result<Self> {
        let config: Config = yaml::parse(FILE, text)?;
        config.check()?;
        Ok(config)
    }

    /// Image the slots run, in the form podman reports. `image` naming a file on
    /// trunk is one `init` builds, anything else an image podman resolves.
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
        self.agents.keys().cloned().collect::<Vec<_>>().join(", ")
    }

    fn check(&self) -> Result<()> {
        if self.image.trim().is_empty() {
            return Err(invalid(
                "field `image` must name a Containerfile path or an image",
            ));
        }
        if self.commit.trim().is_empty() {
            return Err(invalid(
                "field `commit` must hold the turn that asks for a commit message",
            ));
        }
        self.check_agents()?;
        self.check_merge()
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
        if !self.knows_agent(&self.default_agent) {
            return Err(invalid(&format!(
                "field `default_agent` must name a key of `agents`: {}",
                self.agent_keys()
            )));
        }
        Ok(())
    }

    fn check_merge(&self) -> Result<()> {
        let Some(turns) = self.merge.as_deref() else {
            return Ok(());
        };
        if turns.is_empty() {
            return Err(invalid(
                "field `merge` must hold one turn or more, or stay absent",
            ));
        }
        match turns.iter().position(|turn| turn.trim().is_empty()) {
            Some(index) => Err(invalid(&format!(
                "field `merge` turn {} must hold the text sent to the agent",
                index + 1
            ))),
            None => Ok(()),
        }
    }
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
merge:
  - "Resolve every conflict of the merge of {parent} into {id}."
"#;

    #[test]
    fn parses_contract_example() {
        let config = Config::parse(EXAMPLE).expect("example must parse");
        assert_eq!(config.image, "Containerfile.swarm");
        assert!(config.commit.starts_with("Reply with"), "{}", config.commit);
        assert_eq!(config.merge.expect("merge must parse").len(), 1);
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
