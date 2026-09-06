use std::collections::BTreeMap;

use ron::extensions::Extensions;
use serde::Deserialize;

use crate::error::{Error, Result};
use crate::git::{self, Git};

pub const FILE: &str = "sercswarm.ron";

/// Turn text the tool fills in or acts on.
pub const TASK_SLOT: &str = "{task}";
pub const SESSION_SLOT: &str = "{session}";
pub const COMMIT_TURN: &str = "{commit}";

/// Project settings. Only reader of `sercswarm.ron`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub workers: usize,
    pub image: String,
    #[serde(default)]
    pub mounts: Vec<Mount>,
    /// Host environment variables the containers get. Credentials ride here.
    #[serde(default)]
    pub env: Vec<String>,
    pub default_agent: String,
    pub agents: BTreeMap<String, Agent>,
    pub stages: Vec<Stage>,
}

/// One bind: absolute host path, absolute container path. Both sides rw.
#[derive(Debug, Deserialize)]
#[serde(try_from = "String")]
pub struct Mount {
    pub host: String,
    pub guest: String,
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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Agent {
    pub exec: String,
    #[serde(default)]
    pub resume: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stage {
    #[serde(default)]
    pub agent: Option<String>,
    pub turns: Vec<String>,
}

impl Config {
    pub fn from_main(git: &Git) -> Result<Self> {
        let text = git
            .show(git::MAIN, FILE)?
            .ok_or_else(|| invalid(&format!("absent from branch {}", git::MAIN)))?;
        Self::parse(&text)
    }

    pub fn parse(text: &str) -> Result<Self> {
        let config: Config = ron::Options::default()
            .with_default_extension(Extensions::IMPLICIT_SOME)
            .from_str(text)
            .map_err(|why| Error::Invalid(format!("{FILE}: {why}")))?;
        config.check()?;
        Ok(config)
    }

    pub fn knows_agent(&self, key: &str) -> bool {
        self.agents.contains_key(key)
    }

    pub fn agent_keys(&self) -> String {
        self.agents.keys().cloned().collect::<Vec<_>>().join(", ")
    }

    fn check(&self) -> Result<()> {
        if self.workers == 0 {
            return Err(invalid("field `workers` must be a count of 1 or more"));
        }
        if self.image.trim().is_empty() {
            return Err(invalid(
                "field `image` must name a Containerfile path or an image",
            ));
        }
        self.check_env()?;
        self.check_agents()?;
        self.check_stages()
    }

    fn check_env(&self) -> Result<()> {
        for (index, name) in self.env.iter().enumerate() {
            let plain = |letter: char| letter.is_ascii_alphanumeric() || letter == '_';
            if name.is_empty() || !name.chars().all(plain) {
                return Err(invalid(&format!(
                    "field `env` entry {} must name a host environment variable",
                    index + 1
                )));
            }
        }
        Ok(())
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

    fn check_stages(&self) -> Result<()> {
        if self.stages.is_empty() {
            return Err(invalid("field `stages` must hold one stage or more"));
        }
        for (index, stage) in self.stages.iter().enumerate() {
            let number = index + 1;
            if stage.turns.is_empty() {
                return Err(invalid(&format!(
                    "stage {number} field `turns` must hold one turn or more"
                )));
            }
            if !stage.turns.iter().any(|turn| turn == COMMIT_TURN) {
                return Err(invalid(&format!(
                    "stage {number} field `turns` must hold the turn `{COMMIT_TURN}`, the one commit of the stage"
                )));
            }
            if stage
                .agent
                .as_deref()
                .is_some_and(|key| !self.knows_agent(key))
            {
                return Err(invalid(&format!(
                    "stage {number} field `agent` must name a key of `agents`: {}",
                    self.agent_keys()
                )));
            }
        }
        Ok(())
    }
}

fn invalid(what: &str) -> Error {
    Error::Invalid(format!("{FILE}: {what}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = r#"
(
  workers: 3,
  image: "Containerfile.swarm",
  mounts: ["~/alpha/records:/records"],
  env: ["ANTHROPIC_API_KEY"],
  default_agent: "claude",
  agents: {
    "claude": (
      exec: "claude --model opus -p --output-format json",
      resume: "--resume {session}",
    ),
  },
  stages: [
    (turns: ["/execute {task}", "{commit}"]),
    (agent: "claude", turns: ["/audit {task}", "{commit}"]),
  ],
)
"#;

    #[test]
    fn parses_contract_example() {
        let config = Config::parse(EXAMPLE).expect("example must parse");
        assert_eq!(config.workers, 3);
        assert_eq!(config.mounts[0].host, "~/alpha/records");
        assert_eq!(config.mounts[0].guest, "/records");
        assert_eq!(config.env, ["ANTHROPIC_API_KEY"]);
        assert_eq!(config.stages.len(), 2);
        assert_eq!(config.stages[0].agent, None);
        assert_eq!(config.stages[1].agent.as_deref(), Some("claude"));
        assert_eq!(
            config.agents["claude"].resume.as_deref(),
            Some("--resume {session}")
        );
    }

    #[test]
    fn names_the_bad_field() {
        let text = EXAMPLE.replace("default_agent: \"claude\"", "default_agent: \"codex\"");
        let why = Config::parse(&text)
            .expect_err("unknown agent must fail")
            .to_string();
        assert!(why.contains("field `default_agent`"), "{why}");
    }
}
