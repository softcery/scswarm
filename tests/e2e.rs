//! Live slot set, `serve` restart and two tracks against rootless podman and a local remote.
//! Rows hold `run` and literal commit steps only: no agent call, no credential, no token spent.

use std::collections::BTreeSet;
use std::env;
use std::fs::{self, OpenOptions};
use std::path::PathBuf;
use std::process::{Child, Command, Output};
use std::thread;
use std::time::{Duration, Instant};

const PROJECT: &str = "scswarm-e2e";
/// Track `a` starts from `main` and runs first; track `b` starts from `side`.
const ROWS: [&str; 3] = ["a/row-1", "a/row-2", "a/row-3"];
const OTHER: &str = "b/other-1";
const ROW_SECONDS: u64 = 10;
const WAIT: Duration = Duration::from_secs(180);
const TICK: Duration = Duration::from_millis(200);
/// Host file `poll` is 1 s: two cycles and a reload fit.
const CYCLES: Duration = Duration::from_secs(3);
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
    reports_stale_once(&swarm);
    survives_bad_track(&swarm);
    rolls_image(&swarm);
    let restarted = outlives_serve(&mut swarm);
    pushes_every_track(&swarm, restarted);
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

/// 1 → 3 with rows waiting: new slots take them on the next cycle, priority first.
fn grows(swarm: &Swarm) {
    let from = swarm.wait(&format!("assign row={} slot=0", ROWS[0]), 0);
    swarm.host(3, &[]);
    for needle in [
        "create slot=1",
        "create slot=2",
        &format!("assign row={} ", ROWS[1]),
        &format!("assign row={} ", ROWS[2]),
    ] {
        swarm.wait(needle, from);
    }
}

/// 3 → 1 mid-row: busy slots stay, each drops once its row lands. The track of
/// lower priority runs once a slot frees.
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
    swarm.wait(&format!("land row={}", ROWS[0]), from);
    swarm.wait(&format!("push row={OTHER} "), from);
}

/// 3 → 1 → 3 inside one row: nothing drops, nothing rebuilds.
fn holds(swarm: &Swarm) {
    let from = swarm.mark();
    swarm.host(3, &[]);
    swarm.rerun(&ROWS, from);
    swarm.wait("create slot=1", from);
    swarm.wait("create slot=2", from);
    let before = swarm.containers();
    let flip = swarm.mark();
    swarm.host(1, &[]);
    thread::sleep(CYCLES);
    swarm.host(3, &[]);
    for row in ROWS {
        swarm.wait(&format!("land row={row}"), flip);
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
    swarm.rerun(&ROWS[..1], from);
    swarm.wait(&format!("land row={}", ROWS[0]), from);
    swarm.quiet(from);
    assert_eq!(
        swarm.containers(),
        before,
        "`env` edit must keep every container"
    );
}

/// Broken host file across cycles: one `stale` line, last good copy runs on.
fn reports_stale_once(swarm: &Swarm) {
    let from = swarm.mark();
    fs::write(host_file(), "workers: [").expect("host file must be writable");
    thread::sleep(CYCLES);
    let stale = swarm.lines()[from..]
        .iter()
        .filter(|line| line.starts_with("stale "))
        .count();
    assert_eq!(stale, 1, "a repeated host file fault must report once");
    swarm.host(3, &["HOME"]);
    thread::sleep(CYCLES);
    swarm.quiet(from);
}

/// Broken track file pushed mid-row: one `skip`, the row lands, a fixed file resumes.
fn survives_bad_track(swarm: &Swarm) {
    let from = swarm.mark();
    swarm.rerun(&ROWS[..1], from);
    swarm.commit_on(
        "track/a",
        "SWARM.yaml",
        "from: main\nto: [\n",
        "e2e: broken track",
    );
    swarm.wait("skip cause=track `a`", from);
    swarm.wait(&format!("land row={}", ROWS[0]), from);
    thread::sleep(CYCLES);
    let skips = swarm.lines()[from..]
        .iter()
        .filter(|line| line.starts_with("skip "))
        .count();
    assert_eq!(skips, 1, "a repeated track fault must report once");
    swarm.commit_on("track/a", "SWARM.yaml", &track_a(), "e2e: track fixed");
    let fixed = swarm.mark();
    swarm.wait("fetch track=a ", fixed);
    swarm.rerun(&ROWS[1..2], fixed);
    swarm.wait(&format!("push row={} ", ROWS[1]), fixed);
    swarm.quiet(from);
}

/// Containerfile change pushed on `to`: each slot builds it and remakes its
/// container at its next row, and all end on one new image.
fn rolls_image(swarm: &Swarm) {
    let old = swarm.images();
    swarm.commit_on(
        "feat/a",
        CONTAINERFILE,
        &containerfile(2),
        "e2e: image change",
    );
    let from = swarm.mark();
    swarm.rerun(&ROWS, from);
    for slot in 0..3 {
        swarm.wait(&format!("create slot={slot}"), from);
    }
    for row in ROWS {
        swarm.wait(&format!("push row={row} "), from);
    }
    let new = swarm.images();
    assert_eq!(new.len(), 1, "every slot must run one image: {new:?}");
    assert!(
        old.is_disjoint(&new),
        "Containerfile change must build a new image ID"
    );
}

/// SIGKILL on `serve` mid-row: the worker runs on, a new `serve` waits on its
/// locks. The row lands once, one try per step. Returns the restart line.
fn outlives_serve(swarm: &mut Swarm) -> usize {
    let row = ROWS[0];
    let from = swarm.mark();
    swarm.rerun(&[row], from);
    swarm.wait(&format!("step row={row} step=1"), from);
    let before = swarm.containers();
    swarm.kill();
    let restarted = swarm.mark();
    swarm.serve();
    swarm.wait("fetch track=", restarted);
    let second = command()
        .args(["serve", PROJECT])
        .output()
        .expect("second `scswarm serve` must start");
    assert_eq!(second.status.code(), Some(2), "second `serve` must exit 2");
    let why = String::from_utf8_lossy(&second.stderr);
    assert!(why.contains("another `scswarm serve"), "{why}");
    swarm.wait(&format!("land row={row}"), from);
    let count = |needle: &str| {
        swarm.lines()[from..]
            .iter()
            .filter(|line| line.starts_with(needle))
            .count()
    };
    assert_eq!(
        count(&format!("assign row={row} ")),
        1,
        "row must not reassign"
    );
    assert_eq!(count(&format!("step row={row} ")), 1, "row must not replay");
    assert_eq!(
        count(&format!("attempt row={row} ")),
        0,
        "step must take one try"
    );
    assert_eq!(
        swarm.containers(),
        before,
        "restart must keep every container"
    );
    restarted
}

/// Two tracks from two branches each push a `to` that holds every row, and a
/// plain clone reads every row as `pushed`.
fn pushes_every_track(swarm: &Swarm, from: usize) {
    let pushed = swarm.wait(&format!("push row={} ", ROWS[0]), from);
    swarm.wait("mirror refs=", pushed);
    let clone = scratch().join("clone");
    let remote = swarm.remote();
    succeed(
        git().args(["clone", "--quiet"]).arg(&remote).arg(&clone),
        "git",
    );
    succeed(
        git().arg("-C").arg(&clone).args([
            "fetch",
            "--quiet",
            "origin",
            "+refs/swarm/*:refs/swarm/*",
        ]),
        "git",
    );
    let table = succeed(
        command()
            .args(["status", "--tracks", "track/"])
            .current_dir(&clone),
        "scswarm",
    );
    for row in ROWS.iter().chain([&OTHER]) {
        let line = table
            .lines()
            .find(|line| line.starts_with(&format!("{row} ")))
            .unwrap_or_else(|| panic!("status must list {row}:\n{table}"));
        assert!(
            line.contains(" pushed "),
            "{row} must read pushed:\n{table}"
        );
    }
}

/// Scratch project: bare remote, a clone that pushes to it, host file, `serve`
/// log. Drop removes every trace, pass or fail.
struct Swarm {
    work: PathBuf,
    log: PathBuf,
    serve: Option<Child>,
}

impl Swarm {
    fn open() -> Self {
        clean();
        let root = scratch();
        fs::create_dir_all(&root).expect("scratch dir must be writable");
        let swarm = Self {
            work: root.join("work"),
            log: root.join("serve.log"),
            serve: None,
        };
        succeed(
            git()
                .args(["init", "--quiet", "--bare"])
                .arg(swarm.remote()),
            "git",
        );
        succeed(
            git()
                .args(["clone", "--quiet"])
                .arg(swarm.remote())
                .arg(&swarm.work),
            "git",
        );
        swarm.git(&["checkout", "--quiet", "--orphan", "main"]);
        swarm.write("scswarm.yaml", CONFIG);
        swarm.commit_here(CONTAINERFILE, &containerfile(1), "e2e: project", "main");
        swarm.git(&["checkout", "--quiet", "-b", "side"]);
        swarm.commit_here("side.txt", "side\n", "e2e: side", "side");
        swarm.track("track/a", &track_a());
        swarm.track("track/b", &track_b());
        swarm
    }

    fn remote(&self) -> PathBuf {
        scratch().join("remote.git")
    }

    fn host(&self, workers: usize, env: &[&str]) {
        let file = host_file();
        fs::create_dir_all(file.parent().expect("host file has a dir"))
            .expect("config dir must be writable");
        let rules = match env.is_empty() {
            true => String::new(),
            false => format!("env:\n  - match: '*'\n    names: [{}]\n", env.join(", ")),
        };
        let text = format!(
            "remote: {}\ntracks: track/\nworkers: {workers}\n{rules}poll: 1\n",
            self.remote().display()
        );
        fs::write(&file, text).expect("host file must be writable");
    }

    /// Log opens in append mode: a worker that outlives its `serve` writes to the
    /// end too.
    fn serve(&mut self) {
        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log)
            .expect("log must be writable");
        let child = command()
            .args(["serve", PROJECT])
            .stdout(log.try_clone().expect("log handle must clone"))
            .stderr(log)
            .spawn()
            .expect("`scswarm serve` must start");
        self.serve = Some(child);
    }

    /// SIGKILL: no cleanup runs in `serve`.
    fn kill(&mut self) {
        let mut serve = self.serve.take().expect("`serve` must run");
        serve.kill().expect("`serve` must take SIGKILL");
        serve.wait().expect("`serve` must be reaped");
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
            self.wait(&format!("assign row={row} "), from);
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
                let row = line.strip_prefix("assign row=")?.strip_suffix(&tail)?;
                Some((index, row.to_string()))
            })
            .unwrap_or_else(|| panic!("log must show a row assigned to slot {slot}"));
        self.wait(&format!("land row={row}"), assigned)
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

    /// Image IDs the slot containers run.
    fn images(&self) -> BTreeSet<String> {
        self.containers()
            .iter()
            .filter_map(|line| line.rsplit(' ').next().map(str::to_string))
            .collect()
    }

    /// Orphan track branch holding `text` as its track file, pushed.
    fn track(&self, branch: &str, text: &str) {
        self.git(&["checkout", "--quiet", "--orphan", branch]);
        self.git(&["rm", "-r", "--quiet", "--force", "--cached", "."]);
        self.git(&["clean", "--quiet", "--force", "-d"]);
        self.commit_here("SWARM.yaml", text, "e2e: track", branch);
    }

    /// Commit `text` as `file` on the remote tip of `branch`, then push it.
    fn commit_on(&self, branch: &str, file: &str, text: &str, message: &str) {
        self.git(&["fetch", "--quiet", "origin"]);
        let tip = format!("origin/{branch}");
        self.git(&["checkout", "--quiet", "--force", "-B", branch, &tip]);
        self.commit_here(file, text, message, branch);
    }

    fn commit_here(&self, file: &str, text: &str, message: &str, branch: &str) {
        self.write(file, text);
        self.git(&["add", "--all"]);
        self.git(&["commit", "--quiet", "--message", message]);
        self.git(&["push", "--quiet", "origin", branch]);
    }

    fn write(&self, file: &str, text: &str) {
        fs::write(self.work.join(file), text).expect("scratch file must be writable");
    }

    fn git(&self, args: &[&str]) {
        succeed(git().arg("-C").arg(&self.work).args(args), "git");
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
commit: \"unused: rows hold literal commits only\"
";

fn track_a() -> String {
    track("main", "feat/a", 0, &ROWS)
}

/// Lower priority: runs once track `a` leaves a slot free.
fn track_b() -> String {
    track("side", "feat/b", -1, &[OTHER])
}

fn track(from: &str, to: &str, priority: i32, rows: &[&str]) -> String {
    let rows: String = rows
        .iter()
        .map(|key| {
            let id = key.split_once('/').expect("key holds a `/`").1;
            format!(
                "  - id: {id}\n    steps:\n      - run: sleep {ROW_SECONDS}\n      - commit: \"{id}\"\n"
            )
        })
        .collect();
    format!("from: {from}\nto: {to}\npriority: {priority}\nrows:\n{rows}")
}

fn containerfile(revision: u32) -> String {
    format!("FROM {BASE}\nENV E2E_REVISION={revision}\n")
}

/// Every trace of the project: slots, images, bare repos, logs, host file, scratch.
fn clean() {
    let listing = Command::new("podman")
        .args([
            "ps",
            "--all",
            "--format={{.ID}} {{.ImageID}}",
            &format!("--filter=name=^scswarm-{PROJECT}-"),
        ])
        .output()
        .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
        .unwrap_or_default();
    for line in listing.lines() {
        let mut fields = line.split(' ');
        if let (Some(id), Some(image)) = (fields.next(), fields.next()) {
            let _ = Command::new("podman").args(["rm", "--force", id]).output();
            let _ = Command::new("podman")
                .args(["rmi", "--force", image])
                .output();
        }
    }
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
