//! Live slot set against rootless podman. Board holds `run` and literal commit
//! steps only: no agent call, no credential, no token spent.

use std::env;
use std::fs::{self, File};
use std::path::PathBuf;
use std::process::{Child, Command, Output};
use std::thread;
use std::time::{Duration, Instant};

const PROJECT: &str = "scswarm-e2e";
const ROWS: [&str; 3] = ["row-1", "row-2", "row-3"];
const ROW_SECONDS: u64 = 10;
const WAIT: Duration = Duration::from_secs(180);
const TICK: Duration = Duration::from_millis(200);
/// Host file `poll` is 1 s: two cycles and a reload fit.
const CYCLES: Duration = Duration::from_secs(3);
const IMAGE: &str = "localhost/scswarm/scswarm-e2e:latest";
const CONTAINERFILE: &str = "Containerfile.e2e";
const BASE: &str = "registry.fedoraproject.org/fedora:44";

#[test]
#[ignore = "needs Linux and rootless podman: run `make e2e`"]
fn slot_set_follows_host_file_live() {
    let mut swarm = Swarm::open();
    swarm.host(1, &[]);
    swarm.tool(&["init", PROJECT]);
    assert!(
        swarm.containers().is_empty(),
        "`init` must create no container"
    );
    swarm.serve();
    swarm.wait("create slot=0", 0);
    grows(&swarm);
    drains(&swarm);
    holds(&swarm);
    keeps_containers_on_env(&swarm);
    rolls_image(&swarm);
    let creates = swarm
        .lines()
        .iter()
        .filter(|line| line.starts_with("create "))
        .count();
    assert_eq!(
        creates, 8,
        "each `create` must answer a change: 1 + 2 + 2 + 3"
    );
}

/// 1 → 3 with rows waiting: new slots open on the next cycle and take them.
fn grows(swarm: &Swarm) {
    let from = swarm.wait("assign task=row-1 slot=0", 0);
    swarm.host(3, &[]);
    for needle in [
        "create slot=1",
        "create slot=2",
        "assign task=row-2",
        "assign task=row-3",
    ] {
        swarm.wait(needle, from);
    }
}

/// 3 → 1 mid-row: busy slots stay, each drops once its row lands.
fn drains(swarm: &Swarm) {
    let from = swarm.mark();
    swarm.host(1, &[]);
    thread::sleep(CYCLES);
    assert_eq!(
        swarm.containers().len(),
        3,
        "busy slots must outlive the shrink"
    );
    for slot in [1, 2] {
        let landed = swarm.landed_on(slot);
        let removed = swarm.wait(&format!("remove slot={slot}"), from);
        assert!(
            removed > landed,
            "slot {slot} must drop after its row lands"
        );
    }
    swarm.wait("land task=row-1", from);
}

/// 3 → 1 → 3 inside one row: nothing drops, nothing rebuilds.
fn holds(swarm: &Swarm) {
    let from = swarm.mark();
    swarm.host(3, &[]);
    swarm.wait("create slot=1", from);
    swarm.wait("create slot=2", from);
    swarm.rerun(&ROWS, from);
    let before = swarm.containers();
    let flip = swarm.mark();
    swarm.host(1, &[]);
    thread::sleep(CYCLES);
    swarm.host(3, &[]);
    for row in ROWS {
        swarm.wait(&format!("land task={row}"), flip);
    }
    thread::sleep(CYCLES);
    swarm.quiet(flip);
    assert_eq!(swarm.containers(), before, "flip must keep every container");
}

/// `env` rides in each job: an edit rebuilds nothing.
fn keeps_containers_on_env(swarm: &Swarm) {
    let before = swarm.containers();
    let from = swarm.mark();
    swarm.host(3, &["HOME"]);
    thread::sleep(CYCLES);
    swarm.rerun(&["row-1"], from);
    swarm.wait("land task=row-1", from);
    swarm.quiet(from);
    assert_eq!(
        swarm.containers(),
        before,
        "`env` edit must keep every container"
    );
}

/// `init` during `serve` with a changed image: each slot rebuilds once its row
/// lands, and all end on the new image.
fn rolls_image(swarm: &Swarm) {
    let old = swarm.image();
    swarm.commit(CONTAINERFILE, &containerfile(2), "e2e: image change");
    let from = swarm.mark();
    swarm.rerun(&ROWS, from);
    swarm.tool(&["init", PROJECT]);
    let new = swarm.image();
    assert_ne!(old, new, "Containerfile change must build a new image ID");
    for slot in 0..3 {
        let landed = swarm.landed_on(slot);
        let created = swarm.wait(&format!("create slot={slot}"), from);
        assert!(
            created > landed,
            "slot {slot} must rebuild after its row lands"
        );
    }
    let start = Instant::now();
    while !swarm.containers().iter().all(|line| line.ends_with(&new)) {
        assert!(start.elapsed() < WAIT, "every slot must reach image {new}");
        thread::sleep(TICK);
    }
}

/// Scratch project: checkout, host file, `serve` log. Drop removes every trace,
/// pass or fail.
struct Swarm {
    repo: PathBuf,
    log: PathBuf,
    serve: Option<Child>,
}

impl Swarm {
    fn open() -> Self {
        clean();
        let repo = scratch();
        fs::create_dir_all(&repo).expect("scratch dir must be writable");
        let swarm = Self {
            log: repo.with_extension("log"),
            repo,
            serve: None,
        };
        swarm.git(&["init", "--quiet", "--initial-branch=main"]);
        swarm.write("scswarm.yaml", CONFIG);
        swarm.write("SWARM.yaml", &board());
        swarm.commit(CONTAINERFILE, &containerfile(1), "e2e: project");
        swarm
    }

    fn host(&self, workers: usize, env: &[&str]) {
        let file = host_file();
        fs::create_dir_all(file.parent().expect("host file has a dir"))
            .expect("config dir must be writable");
        let text = format!(
            "path: {}\ntrunk: main\nworkers: {workers}\nenv: [{}]\npoll: 1\n",
            self.repo.display(),
            env.join(", ")
        );
        fs::write(&file, text).expect("host file must be writable");
    }

    fn serve(&mut self) {
        let log = File::create(&self.log).expect("log must be writable");
        let child = command()
            .args(["serve", PROJECT])
            .stdout(log.try_clone().expect("log handle must clone"))
            .stderr(log)
            .spawn()
            .expect("`scswarm serve` must start");
        self.serve = Some(child);
    }

    fn tool(&self, args: &[&str]) {
        succeed(command().args(args), "scswarm");
    }

    /// Reset `rows`, wait until a slot takes each.
    fn rerun(&self, rows: &[&str], from: usize) {
        for row in rows {
            self.tool(&["reset", PROJECT, row]);
        }
        for row in rows {
            self.wait(&format!("assign task={row} "), from);
        }
    }

    fn lines(&self) -> Vec<String> {
        fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn mark(&self) -> usize {
        self.lines().len()
    }

    /// Index of first line at or after `from` holding `needle`.
    fn wait(&self, needle: &str, from: usize) -> usize {
        let start = Instant::now();
        loop {
            let lines = self.lines();
            if let Some(found) = lines
                .iter()
                .skip(from)
                .position(|line| line.contains(needle))
            {
                return from + found;
            }
            if start.elapsed() > WAIT {
                let tail = lines[lines.len().saturating_sub(20)..].join("\n");
                panic!("log must show `{needle}` within {WAIT:?}; last lines:\n{tail}");
            }
            thread::sleep(TICK);
        }
    }

    /// Line where the last row assigned to `slot` lands.
    fn landed_on(&self, slot: usize) -> usize {
        let lines = self.lines();
        let tail = format!(" slot={slot}");
        let (assigned, row) = lines
            .iter()
            .enumerate()
            .rev()
            .find_map(|(index, line)| {
                let row = line.strip_prefix("assign task=")?.strip_suffix(&tail)?;
                Some((index, row.to_string()))
            })
            .unwrap_or_else(|| panic!("log must show a row assigned to slot {slot}"));
        self.wait(&format!("land task={row}"), assigned)
    }

    /// No slot created or removed since `from`.
    fn quiet(&self, from: usize) {
        let changed: Vec<String> = self.lines()[from..]
            .iter()
            .filter(|line| line.starts_with("create ") || line.starts_with("remove "))
            .cloned()
            .collect();
        assert!(changed.is_empty(), "slot set must stand still: {changed:?}");
    }

    /// `name id image-id` per slot container, sorted.
    fn containers(&self) -> Vec<String> {
        let filter = format!("--filter=name=^scswarm-{PROJECT}-");
        let format = "--format={{.Names}} {{.ID}} {{.ImageID}}";
        let out = succeed(
            Command::new("podman").args(["ps", "--all", "--no-trunc", &filter, format]),
            "podman",
        );
        let mut held: Vec<String> = out.lines().map(str::to_string).collect();
        held.sort();
        held
    }

    fn image(&self) -> String {
        let out = succeed(
            Command::new("podman").args(["image", "inspect", "--format={{.Id}}", IMAGE]),
            "podman",
        );
        out.trim().to_string()
    }

    fn commit(&self, file: &str, text: &str, message: &str) {
        self.write(file, text);
        self.git(&["add", "--all"]);
        self.git(&["commit", "--quiet", "--message", message]);
    }

    fn write(&self, file: &str, text: &str) {
        fs::write(self.repo.join(file), text).expect("scratch file must be writable");
    }

    fn git(&self, args: &[&str]) {
        succeed(git().arg("-C").arg(&self.repo).args(args), "git");
    }
}

impl Drop for Swarm {
    fn drop(&mut self) {
        if let Some(mut serve) = self.serve.take() {
            let _ = serve.kill();
            let _ = serve.wait();
        }
        clean();
    }
}

const CONFIG: &str = "\
image: Containerfile.e2e
default_agent: none
agents:
  none:
    exec: \"false\"
commit: \"unused: board holds literal commits only\"
";

fn board() -> String {
    ROWS.iter()
        .map(|row| {
            format!(
                "- id: {row}\n  steps:\n    - run: sleep {ROW_SECONDS}\n    - commit: \"{row}\"\n"
            )
        })
        .collect()
}

fn containerfile(revision: u32) -> String {
    format!("FROM {BASE}\nENV E2E_REVISION={revision}\n")
}

/// Every trace of the project: slots, image, bare repo, logs, host file, checkout.
fn clean() {
    let names = Command::new("podman")
        .args([
            "ps",
            "--all",
            "--quiet",
            &format!("--filter=name=^scswarm-{PROJECT}-"),
        ])
        .output()
        .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
        .unwrap_or_default();
    for id in names.lines() {
        let _ = Command::new("podman").args(["rm", "--force", id]).output();
    }
    let _ = Command::new("podman")
        .args(["rmi", "--force", IMAGE])
        .output();
    let swarm = home().join("swarm");
    for dir in [
        swarm.join(format!("{PROJECT}.git")),
        swarm.join("slot").join(PROJECT),
        swarm.join("log").join(PROJECT),
        swarm.join("build").join(PROJECT),
        scratch(),
    ] {
        let _ = fs::remove_dir_all(dir);
    }
    let _ = fs::remove_file(host_file());
    let _ = fs::remove_file(scratch().with_extension("log"));
}

/// Tool under test. Git identity set here: host commits need one, the machine may
/// lack it.
fn command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_scswarm"));
    identity(&mut command);
    command
}

fn git() -> Command {
    let mut command = Command::new("git");
    identity(&mut command);
    command
}

fn identity(command: &mut Command) {
    for (key, value) in [
        ("GIT_AUTHOR_NAME", "scswarm-e2e"),
        ("GIT_AUTHOR_EMAIL", "e2e@localhost"),
        ("GIT_COMMITTER_NAME", "scswarm-e2e"),
        ("GIT_COMMITTER_EMAIL", "e2e@localhost"),
    ] {
        command.env(key, value);
    }
}

fn succeed(command: &mut Command, name: &str) -> String {
    let Output {
        status,
        stdout,
        stderr,
    } = command
        .output()
        .unwrap_or_else(|why| panic!("{name} must start: {why}"));
    assert!(
        status.success(),
        "{name} must exit 0: {status}\n{}",
        String::from_utf8_lossy(&stderr)
    );
    String::from_utf8_lossy(&stdout).into_owned()
}

fn scratch() -> PathBuf {
    env::temp_dir().join(PROJECT)
}

fn host_file() -> PathBuf {
    let config = env::var_os("XDG_CONFIG_HOME")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".config"));
    config.join("scswarm").join(format!("{PROJECT}.yaml"))
}

fn home() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .expect("HOME must hold the home dir")
}
