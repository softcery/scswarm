use std::fmt;
use std::time::Duration;

use serde::de::{self, value::MapAccessDeserializer, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};

/// Tool defaults, last of three layers: step field, then `defaults`, then these.
/// Every agent try spends budget; a script gives the same result twice.
const PROMPT_TRIES: usize = 3;
const RUN_TRIES: usize = 1;
const WAIT: u64 = 1;
const WAIT_MAX: u64 = 60;

/// One step of a session, settled: every policy field resolved.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// Agent turn in the row session.
    Prompt { text: String, policy: Policy },
    /// Shell text in the slot container, worktree as working dir.
    Run { command: String, policy: Policy },
    /// Agent writes message in row session, host commits.
    AgentCommit { policy: Policy },
    /// Host commits with message as written.
    Commit { message: String },
}

/// How an exec'd step runs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Policy {
    pub tries: usize,
    /// Pause after a failed try. Doubles per try up to `wait_max`.
    pub wait: Duration,
    pub wait_max: Duration,
    /// Try killed past this. `None`: no cap.
    pub timeout: Option<Duration>,
    /// Spent tries move on to the next step instead of failing the row.
    pub continue_on_error: bool,
}

/// Policy as written in `defaults` or on one step. Absent field: next layer decides.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tuning {
    #[serde(default)]
    tries: Option<usize>,
    #[serde(default)]
    wait: Option<u64>,
    #[serde(default)]
    wait_max: Option<u64>,
    #[serde(default)]
    timeout: Option<u64>,
    #[serde(default)]
    continue_on_error: Option<bool>,
}

/// Field `defaults` of `scswarm.yaml`: one policy per exec'd step kind.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Defaults {
    #[serde(default)]
    prompt: Tuning,
    #[serde(default)]
    run: Tuning,
}

/// Step as written. String = prompt; map names kind by key.
#[derive(Debug)]
pub struct Written(Fields);

/// Step kinds that exec in the container. Agent commit is a prompt.
#[derive(Clone, Copy)]
enum Kind {
    Prompt,
    Run,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Fields {
    #[serde(default)]
    prompt: Option<String>,
    #[serde(default)]
    run: Option<String>,
    /// `Some(None)`: key present with no value, the agent writes the message.
    #[serde(default, deserialize_with = "present")]
    commit: Option<Option<String>>,
    #[serde(flatten)]
    tuning: Tuning,
}

/// Settle a step list against `defaults`. Error names `field`, step number and
/// step field; caller names the file and row.
pub fn settle(field: &str, written: &[Written], defaults: &Defaults) -> Result<Vec<Step>, String> {
    let steps = written
        .iter()
        .enumerate()
        .map(|(at, step)| {
            step.0
                .settle(defaults)
                .map_err(|what| format!("field `{field}` step {}: {what}", at + 1))
        })
        .collect::<Result<Vec<Step>, String>>()?;
    match steps.last() {
        None => Err(format!("field `{field}` must hold one step or more")),
        Some(Step::AgentCommit { .. } | Step::Commit { .. }) => Ok(steps),
        Some(_) => Err(format!("field `{field}` must end with a `commit` step")),
    }
}

impl Defaults {
    pub fn check(&self) -> Result<(), String> {
        self.prompt.settle(Kind::Prompt, "defaults.prompt.")?;
        self.run.settle(Kind::Run, "defaults.run.").map(drop)
    }

    fn of(&self, kind: Kind) -> &Tuning {
        match kind {
            Kind::Prompt => &self.prompt,
            Kind::Run => &self.run,
        }
    }
}

impl Fields {
    fn settle(&self, defaults: &Defaults) -> Result<Step, String> {
        let policy = |kind| self.tuning.over(defaults.of(kind)).settle(kind, "");
        match (&self.prompt, &self.run, &self.commit) {
            (Some(text), None, None) => Ok(Step::Prompt {
                text: filled("prompt", text)?.to_string(),
                policy: policy(Kind::Prompt)?,
            }),
            (None, Some(command), None) => Ok(Step::Run {
                command: filled("run", command)?.to_string(),
                policy: policy(Kind::Run)?,
            }),
            (None, None, Some(None)) => Ok(Step::AgentCommit {
                policy: policy(Kind::Prompt)?,
            }),
            (None, None, Some(Some(message))) if message.trim().is_empty() => Err(
                "field `commit` must hold the message, or no value for the agent to write it"
                    .to_string(),
            ),
            (None, None, Some(Some(message))) if self.tuning.is_empty() => Ok(Step::Commit {
                message: message.trim().to_string(),
            }),
            (None, None, Some(Some(_))) => {
                Err("a `commit` step with a message must hold no other field".to_string())
            }
            _ => Err("must hold exactly one of the keys `prompt`, `run`, `commit`".to_string()),
        }
    }
}

impl Tuning {
    fn is_empty(&self) -> bool {
        let Self {
            tries,
            wait,
            wait_max,
            timeout,
            continue_on_error,
        } = self;
        tries.is_none()
            && wait.is_none()
            && wait_max.is_none()
            && timeout.is_none()
            && continue_on_error.is_none()
    }

    /// Field-wise: own value, else `base`.
    fn over(&self, base: &Self) -> Self {
        Self {
            tries: self.tries.or(base.tries),
            wait: self.wait.or(base.wait),
            wait_max: self.wait_max.or(base.wait_max),
            timeout: self.timeout.or(base.timeout),
            continue_on_error: self.continue_on_error.or(base.continue_on_error),
        }
    }

    /// Fill gaps with tool defaults and check. `path` prefixes each field name.
    fn settle(&self, kind: Kind, path: &str) -> Result<Policy, String> {
        let field = |name: &str| format!("field `{path}{name}`");
        let (tries, continue_on_error) = match kind {
            Kind::Prompt if self.continue_on_error.is_some() => {
                return Err(format!(
                    "{} belongs to `run` steps alone",
                    field("continue_on_error")
                ))
            }
            Kind::Prompt => (self.tries.unwrap_or(PROMPT_TRIES), false),
            Kind::Run => (
                self.tries.unwrap_or(RUN_TRIES),
                self.continue_on_error.unwrap_or(false),
            ),
        };
        if tries == 0 {
            return Err(format!("{} must be a count of 1 or more", field("tries")));
        }
        if self.timeout == Some(0) {
            return Err(format!(
                "{} must be seconds, 1 or more, or stay absent",
                field("timeout")
            ));
        }
        let wait = self.wait.unwrap_or(WAIT);
        let wait_max = self.wait_max.unwrap_or(WAIT_MAX);
        if wait > wait_max {
            return Err(format!(
                "{} must not exceed {} once both are resolved",
                field("wait"),
                field("wait_max")
            ));
        }
        Ok(Policy {
            tries,
            wait: Duration::from_secs(wait),
            wait_max: Duration::from_secs(wait_max),
            timeout: self.timeout.map(Duration::from_secs),
            continue_on_error,
        })
    }
}

impl<'de> Deserialize<'de> for Written {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(WrittenVisitor)
    }
}

struct WrittenVisitor;

impl<'de> Visitor<'de> for WrittenVisitor {
    type Value = Written;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a prompt string, or a map with key `prompt`, `run` or `commit`")
    }

    fn visit_str<E: de::Error>(self, text: &str) -> Result<Written, E> {
        Ok(Written(Fields {
            prompt: Some(text.to_string()),
            ..Fields::default()
        }))
    }

    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Written, A::Error> {
        Fields::deserialize(MapAccessDeserializer::new(map)).map(Written)
    }
}

/// Key present counts even with no value: `commit:` reads as `Some(None)`.
fn present<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Option<String>>, D::Error> {
    Option::<String>::deserialize(deserializer).map(Some)
}

fn filled<'a>(key: &str, text: &'a str) -> Result<&'a str, String> {
    match text.trim().is_empty() {
        true => Err(format!("field `{key}` must hold text")),
        false => Ok(text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STEPS: &str = r#"
- "Swap the retriever."
- commit:
- run: make bench > bench/out.txt
  tries: 2
  timeout: 1800
- commit: "bench: hybrid retriever"
- run: make test
  continue_on_error: true
- commit:
"#;

    fn settled(text: &str, defaults: &Defaults) -> Result<Vec<Step>, String> {
        let written: Vec<Written> = serde_saphyr::from_str(text).map_err(|why| why.to_string())?;
        settle("steps", &written, defaults)
    }

    #[test]
    fn settles_each_kind_through_three_layers() {
        let defaults: Defaults =
            serde_saphyr::from_str("run: { timeout: 7200, wait: 5 }").expect("defaults must parse");
        let steps = settled(STEPS, &defaults).expect("steps must settle");
        assert_eq!(steps.len(), 6);
        let Step::Prompt { policy, .. } = &steps[0] else {
            panic!("step 1 must be a prompt: {:?}", steps[0]);
        };
        assert_eq!((policy.tries, policy.timeout), (PROMPT_TRIES, None));
        assert!(matches!(steps[1], Step::AgentCommit { .. }));
        let Step::Run { policy, .. } = &steps[2] else {
            panic!("step 3 must be a run: {:?}", steps[2]);
        };
        assert_eq!(policy.tries, 2);
        assert_eq!(policy.timeout, Some(Duration::from_secs(1800)));
        assert_eq!(policy.wait, Duration::from_secs(5));
        assert_eq!(
            steps[3],
            Step::Commit {
                message: "bench: hybrid retriever".to_string()
            }
        );
        let Step::Run { policy, .. } = &steps[4] else {
            panic!("step 5 must be a run: {:?}", steps[4]);
        };
        assert_eq!(policy.timeout, Some(Duration::from_secs(7200)));
        assert!(policy.continue_on_error);
    }

    #[test]
    fn names_the_step_and_field() {
        let text = STEPS.replace("tries: 2", "tries: 0");
        let why = settled(&text, &Defaults::default()).expect_err("zero tries must fail");
        assert_eq!(
            why,
            "field `steps` step 3: field `tries` must be a count of 1 or more"
        );
    }

    #[test]
    fn rejects_a_list_that_ends_without_commit() {
        let why = settled("- run: make bench\n", &Defaults::default())
            .expect_err("no final commit must fail");
        assert_eq!(why, "field `steps` must end with a `commit` step");
    }
}
