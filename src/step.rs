use std::fmt;
use std::ops::Deref;
use std::time::Duration;

use serde::de::{self, value::MapAccessDeserializer, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};

/// Tool defaults, last of three layers: step field, then `defaults`, then these.
/// Every agent try spends budget; a script gives the same result twice.
const PROMPT_TRIES: usize = 3;
const RUN_TRIES: usize = 1;
const WAIT: u64 = 1;
const WAIT_MAX: u64 = 60;

/// Keys that name a step kind, in the order errors list them.
const KEYS: [&str; 3] = ["prompt", "run", "commit"];

/// One step of a session, settled: every policy field resolved.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// Agent turn in the row session.
    Prompt { text: String, policy: Policy },
    /// Shell text in the slot container, worktree as working dir.
    Run {
        command: String,
        policy: Policy,
        /// Spent tries move on to the next step instead of failing the row.
        continue_on_error: bool,
    },
    /// Agent writes message in row session, host commits.
    AgentCommit { policy: Policy },
    /// Host commits with message as written.
    Commit { message: String },
}

/// Settled step list: one step or more, the last a commit. `settle` alone builds it.
#[derive(Debug, Clone, PartialEq)]
pub struct Steps(Vec<Step>);

/// How an exec'd step runs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Policy {
    pub tries: usize,
    /// Pause after a failed try. Doubles per try up to `wait_max`.
    pub wait: Duration,
    pub wait_max: Duration,
    /// Try killed past this. `None`: no cap.
    pub timeout: Option<Duration>,
}

/// How one step, or one step list, ends.
pub enum Outcome<T> {
    Done(T),
    /// Tries spent or conflict left. Text names step and last fault.
    Spent(String),
    /// `scswarm stop` asked: no mark, no rest.
    Stopped,
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

/// Policy as written in `defaults` or on one step. Absent field: next layer decides.
#[derive(Debug, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct Tuning {
    tries: Option<usize>,
    wait: Option<u64>,
    wait_max: Option<u64>,
    timeout: Option<u64>,
    continue_on_error: Option<bool>,
}

/// Step kinds that exec in the container. Agent commit is a prompt.
#[derive(Clone, Copy)]
enum Kind {
    Prompt,
    Run,
}

/// `Some(None)` on a kind key: key present with no value.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Fields {
    #[serde(default, deserialize_with = "present")]
    prompt: Option<Option<String>>,
    #[serde(default, deserialize_with = "present")]
    run: Option<Option<String>>,
    /// No value: the agent writes the message.
    #[serde(default, deserialize_with = "present")]
    commit: Option<Option<String>>,
    #[serde(flatten)]
    tuning: Tuning,
}

/// Settle a step list against `defaults`. Error names `field`, step number and
/// step field; caller names the file and row.
pub fn settle(field: &str, written: &[Written], defaults: &Defaults) -> Result<Steps, String> {
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
        Some(Step::AgentCommit { .. } | Step::Commit { .. }) => Ok(Steps(steps)),
        Some(_) => Err(format!("field `{field}` must end with a `commit` step")),
    }
}

impl Deref for Steps {
    type Target = [Step];

    fn deref(&self) -> &[Step] {
        &self.0
    }
}

impl<T> Outcome<T> {
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Outcome<U> {
        match self {
            Outcome::Done(value) => Outcome::Done(f(value)),
            Outcome::Spent(why) => Outcome::Spent(why),
            Outcome::Stopped => Outcome::Stopped,
        }
    }
}

impl Defaults {
    /// Rules each field states alone. The `wait` pair settles per step.
    pub fn check(&self) -> Result<(), String> {
        self.prompt.check(Kind::Prompt, "defaults.prompt.")?;
        self.run.check(Kind::Run, "defaults.run.")
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
        let policy = |kind| {
            self.tuning.check(kind, "")?;
            self.tuning.over(defaults.of(kind)).policy(kind)
        };
        match (&self.prompt, &self.run, &self.commit) {
            (Some(text), None, None) => Ok(Step::Prompt {
                text: filled("prompt", text)?,
                policy: policy(Kind::Prompt)?,
            }),
            (None, Some(command), None) => Ok(Step::Run {
                command: filled("run", command)?,
                policy: policy(Kind::Run)?,
                continue_on_error: self
                    .tuning
                    .over(&defaults.run)
                    .continue_on_error
                    .unwrap_or(false),
            }),
            (None, None, Some(None)) => Ok(Step::AgentCommit {
                policy: policy(Kind::Prompt)?,
            }),
            (None, None, Some(Some(message))) if message.trim().is_empty() => Err(
                "field `commit` must hold the message, or no value for the agent to write it"
                    .to_string(),
            ),
            (None, None, Some(Some(message))) if self.tuning == Tuning::default() => {
                Ok(Step::Commit {
                    message: message.trim().to_string(),
                })
            }
            (None, None, Some(Some(_))) => {
                Err("a `commit` step with a message must hold no policy field".to_string())
            }
            _ => Err(format!(
                "must hold exactly one of the keys `prompt`, `run`, `commit`, holds {}",
                self.keys()
            )),
        }
    }

    /// Kind keys present, by name. Values stay out of errors.
    fn keys(&self) -> String {
        let held: Vec<String> = [&self.prompt, &self.run, &self.commit]
            .iter()
            .zip(KEYS)
            .filter(|(value, _)| value.is_some())
            .map(|(_, key)| format!("`{key}`"))
            .collect();
        match held.is_empty() {
            true => "none".to_string(),
            false => held.join(", "),
        }
    }
}

impl Tuning {
    /// Rules each field states alone. `path` prefixes each field name.
    fn check(&self, kind: Kind, path: &str) -> Result<(), String> {
        let field = |name: &str| format!("field `{path}{name}`");
        if matches!(kind, Kind::Prompt) && self.continue_on_error.is_some() {
            return Err(format!(
                "{} belongs to `run` steps alone",
                field("continue_on_error")
            ));
        }
        if self.tries == Some(0) {
            return Err(format!("{} must be a count of 1 or more", field("tries")));
        }
        if self.timeout == Some(0) {
            return Err(format!(
                "{} must be seconds, 1 or more, or stay absent",
                field("timeout")
            ));
        }
        Ok(())
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

    /// Fill gaps with tool defaults. Resolved `wait` must not pass `wait_max`.
    fn policy(&self, kind: Kind) -> Result<Policy, String> {
        let wait = self.wait.unwrap_or(WAIT);
        let wait_max = self.wait_max.unwrap_or(WAIT_MAX);
        if wait > wait_max {
            return Err(
                "field `wait` must not exceed field `wait_max` once both are resolved".to_string(),
            );
        }
        Ok(Policy {
            tries: self.tries.unwrap_or(match kind {
                Kind::Prompt => PROMPT_TRIES,
                Kind::Run => RUN_TRIES,
            }),
            wait: Duration::from_secs(wait),
            wait_max: Duration::from_secs(wait_max),
            timeout: self.timeout.map(Duration::from_secs),
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
            prompt: Some(Some(text.to_string())),
            ..Fields::default()
        }))
    }

    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Written, A::Error> {
        Fields::deserialize(MapAccessDeserializer::new(map)).map(Written)
    }
}

/// Key present counts even with no value: `commit:` reads as `Some(None)`.
pub fn present<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Option<String>>, D::Error> {
    Option::<String>::deserialize(deserializer).map(Some)
}

fn filled(key: &str, text: &Option<String>) -> Result<String, String> {
    match text.as_deref().filter(|text| !text.trim().is_empty()) {
        Some(text) => Ok(text.to_string()),
        None => Err(format!("field `{key}` must hold text")),
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

    fn settled(text: &str, defaults: &Defaults) -> Result<Steps, String> {
        let written: Vec<Written> = serde_saphyr::from_str(text).map_err(|why| why.to_string())?;
        settle("steps", &written, defaults)
    }

    fn defaults(text: &str) -> Defaults {
        serde_saphyr::from_str(text).expect("defaults must parse")
    }

    #[test]
    fn settles_each_kind_through_three_layers() {
        let steps = settled(STEPS, &defaults("run: { timeout: 7200, wait: 5 }"))
            .expect("steps must settle");
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
                message: "bench: hybrid retriever".to_string(),
            }
        );
        let Step::Run {
            policy,
            continue_on_error,
            ..
        } = &steps[4]
        else {
            panic!("step 5 must be a run: {:?}", steps[4]);
        };
        assert_eq!(policy.timeout, Some(Duration::from_secs(7200)));
        assert!(continue_on_error);
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
    fn names_a_kind_key_with_no_value() {
        let why = settled("- run:\n- commit:\n", &Defaults::default())
            .expect_err("a `run` with no value must fail");
        assert_eq!(why, "field `steps` step 1: field `run` must hold text");
    }

    #[test]
    fn settles_the_wait_pair_across_layers() {
        let text = "- prompt: Work.\n  wait_max: 300\n- commit:\n  wait_max: 300\n";
        settled(text, &defaults("prompt: { wait: 120 }")).expect("resolved pair must settle");
    }

    #[test]
    fn rejects_a_list_that_ends_without_commit() {
        let why = settled("- run: make bench\n", &Defaults::default())
            .expect_err("no final commit must fail");
        assert_eq!(why, "field `steps` must end with a `commit` step");
    }
}
