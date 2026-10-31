//! Live slot set, `serve` restart, two tracks, two projects and `publish` against rootless podman
//! and local remotes. Rows hold `run` and commit steps, agents are stubs: no token spent.
//! Slots, container names and the host file belong to the user: the test refuses a user whose
//! host file it did not write.

use std::collections::BTreeSet;
use std::env;
use std::fs::{self, OpenOptions};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::Path;
use std::path::PathBuf;
use std::process::{Child, Command, Output};
use std::thread;
use std::time::{Duration, Instant};

const PROJECT: &str = "scswarm-e2e";
/// Second project, own remote: listed once the first one has run.
const SECOND: &str = "scswarm-e2e-second";
/// Track `a` starts from `main` and runs first; track `b` starts from `side`.
const ROWS: [&str; 3] = ["a/row-1", "a/row-2", "a/row-3"];
const OTHER: &str = "b/other-1";
/// Sole row of the second project.
const ELSEWHERE: &str = "c/one";
/// Third project: its remote never exists.
const DEAD: &str = "scswarm-e2e-dead";
/// First line of every host file the test writes. Any other host file is a live swarm.
const MARK: &str = "# scswarm-e2e";
/// Every slot container: `scswarm-<n>`.
const SLOT_NAMES: &str = "^scswarm-[0-9]+$";
const ROW_SECONDS: u64 = 10;
const WAIT: Duration = Duration::from_secs(180);
const TICK: Duration = Duration::from_millis(200);
/// Host file `poll` is 1 s: two cycles and a reload fit.
const CYCLES: Duration = Duration::from_secs(3);
/// Fetch clock of a project whose remote-tracking refs must stand still.
const FETCH_STILL: u32 = 3600;
const CONTAINERFILE: &str = "Containerfile.e2e";
const BASE: &str = "registry.fedoraproject.org/fedora:44";
/// Every row checks this value. The file ends in `\n`: the worker strips it.
const CREDENTIAL: (&str, &str) = ("E2E_CREDENTIAL", "e2e-value");
/// Named by the `env` edit alone.
const EXTRA: (&str, &str) = ("E2E_EXTRA", "extra");

#[test]
#[ignore = "needs rootless podman or a podman machine: run `make e2e`"]
fn slot_set_follows_host_file_live() {
    let mut swarm = Swarm::open();
    swarm.host(1, &[]);
    swarm.serve();
    swarm.wait("create slot=0", 0);
    grows(&swarm);
    drains(&swarm);
    holds(&swarm);
    keeps_containers_on_env(&swarm);
    reports_stale_once(&swarm);
    skips_track_without_credential(&swarm);
    survives_bad_track(&swarm);
    rolls_image(&swarm);
    switches_project(&swarm);
    rests_dead_project(&swarm);
    let restarted = outlives_serve(&mut swarm);
    pushes_every_track(&swarm, restarted);
    publishes_next_board(&swarm);
    publishes_through_agent_commit(&swarm);
    refuses_a_symlink(&swarm);
    publishes_onto_a_new_tip(&swarm);
    fails_when_the_remote_declines(&swarm);
    merges_again_after_publish(&swarm);
    replays_after_a_kill(&swarm);
    stops_on_an_absent_track_branch(&swarm);
    let creates = swarm
        .lines()
        .iter()
        .filter(|line| line.starts_with("create "))
        .count();
    assert_eq!(
        creates, 10,
        "each `create` must answer a change: 1 + 2 + 2 + 3 + 2"
    );
}

/// 1 → 3 with rows waiting: new slots take them on the next cycle, priority first.
fn grows(swarm: &Swarm) {
    let from = swarm.wait(&format!("assign row={} slot=0", label(ROWS[0])), 0);
    swarm.host(3, &[]);
    for needle in [
        "create slot=1",
        "create slot=2",
        &format!("assign row={} ", label(ROWS[1])),
        &format!("assign row={} ", label(ROWS[2])),
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
    swarm.wait(&format!("land row={}", label(ROWS[0])), from);
    swarm.wait(&format!("push row={} ", label(OTHER)), from);
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
        swarm.wait(&format!("land row={}", label(row)), flip);
    }
    thread::sleep(CYCLES);
    swarm.quiet(flip);
    assert_eq!(swarm.containers(), before, "flip must keep every container");
}

/// `env` rides in each job: an edit rebuilds nothing.
fn keeps_containers_on_env(swarm: &Swarm) {
    let before = swarm.containers();
    let from = swarm.mark();
    swarm.host(3, &[EXTRA.0]);
    thread::sleep(CYCLES);
    swarm.rerun(&ROWS[..1], from);
    swarm.wait(&format!("land row={}", label(ROWS[0])), from);
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
    swarm.host(3, &[EXTRA.0]);
    thread::sleep(CYCLES);
    swarm.quiet(from);
}

/// Credential with no file: each track it names skips once, the host file stays good.
fn skips_track_without_credential(swarm: &Swarm) {
    let from = swarm.mark();
    swarm.host(3, &["E2E_MISSING"]);
    let skip = format!("skip project={PROJECT} cause=track `a`");
    swarm.wait(&skip, from);
    thread::sleep(CYCLES);
    let lines = &swarm.lines()[from..];
    let count = |prefix: &str| lines.iter().filter(|line| line.starts_with(prefix)).count();
    assert_eq!(count(&skip), 1, "a missing credential must report once");
    assert_eq!(
        count("stale "),
        0,
        "a missing credential must leave the host file good"
    );
    swarm.host(3, &[EXTRA.0]);
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
    swarm.wait(&format!("skip project={PROJECT} cause=track `a`"), from);
    swarm.wait(&format!("land row={}", label(ROWS[0])), from);
    thread::sleep(CYCLES);
    let skips = swarm.lines()[from..]
        .iter()
        .filter(|line| line.starts_with("skip "))
        .count();
    assert_eq!(skips, 1, "a repeated track fault must report once");
    swarm.commit_on("track/a", "SWARM.yaml", &track_a(), "e2e: track fixed");
    let fixed = swarm.mark();
    swarm.wait(&format!("tip project={PROJECT} track=a "), fixed);
    swarm.rerun(&ROWS[1..2], fixed);
    swarm.wait(&format!("push row={} ", label(ROWS[1])), fixed);
    swarm.quiet(from);
}

/// Containerfile change pushed on `to`: each slot builds it and remakes its
/// container at its next row, and all end on one new image. Rows rerun once
/// `serve` reads the new tip: `reset` acts at once, a fetch on its own clock.
fn rolls_image(swarm: &Swarm) {
    let old = swarm.images();
    let pushed = swarm.mark();
    swarm.commit_on(
        "feat/a",
        CONTAINERFILE,
        &containerfile(2),
        "e2e: image change",
    );
    let tip = swarm.head();
    swarm.wait(&format!("to={}", &tip[..7]), pushed);
    let from = swarm.mark();
    swarm.rerun(&ROWS, from);
    for slot in 0..3 {
        swarm.wait(&format!("create slot={slot}"), from);
    }
    for row in ROWS {
        swarm.wait(&format!("push row={} ", label(row)), from);
    }
    let new = swarm.images();
    assert_eq!(new.len(), 1, "every slot must run one image: {new:?}");
    assert!(
        old.is_disjoint(&new),
        "Containerfile change must build a new image ID"
    );
}

/// Second project listed: its row takes slot 0, whose worktree and container
/// move to the second repo, then a row of the first project moves them back.
/// `status` with no project lists both.
fn switches_project(swarm: &Swarm) {
    let from = swarm.mark();
    swarm.with_second(3, &[EXTRA.0]);
    let row = format!("{SECOND}/{ELSEWHERE}");
    swarm.wait(&format!("assign row={row} slot=0"), from);
    swarm.wait("create slot=0", from);
    swarm.wait(&format!("push row={row} "), from);
    let back = swarm.mark();
    swarm.rerun(&ROWS[..1], back);
    swarm.wait("create slot=0", back);
    swarm.wait(&format!("push row={} ", label(ROWS[0])), back);
    let table = succeed(command().arg("status"), "scswarm");
    for (project, row) in [(PROJECT, ROWS[0]), (SECOND, ELSEWHERE)] {
        assert!(
            table.lines().any(|line| {
                let cells: Vec<&str> = line.split_whitespace().collect();
                cells[..3] == [project, row, "pushed"]
            }),
            "status must list {project} {row} as pushed:\n{table}"
        );
    }
}

/// Third project whose remote is gone: one `skip` while it rests, and the first
/// project lands a row meanwhile.
fn rests_dead_project(swarm: &Swarm) {
    let from = swarm.mark();
    swarm.with_dead(3, &[EXTRA.0]);
    let skip = format!("skip project={DEAD} ");
    swarm.wait(&skip, from);
    swarm.rerun(&ROWS[..1], from);
    swarm.wait(&format!("push row={} ", label(ROWS[0])), from);
    let skips = swarm.lines()[from..]
        .iter()
        .filter(|line| line.starts_with(&skip))
        .count();
    assert_eq!(skips, 1, "a dead remote must report once");
    swarm.with_second(3, &[EXTRA.0]);
    thread::sleep(CYCLES);
}

/// SIGKILL on `serve` mid-row: the worker runs on, a new `serve` waits on its
/// locks. The row lands once, one try per step. Returns the restart line.
fn outlives_serve(swarm: &mut Swarm) -> usize {
    let row = ROWS[0];
    let from = swarm.mark();
    swarm.rerun(&[row], from);
    swarm.wait(&format!("step row={} step=1", label(row)), from);
    let before = swarm.containers();
    swarm.kill();
    let restarted = swarm.mark();
    swarm.serve();
    swarm.wait("tip project=", restarted);
    let second = command()
        .arg("serve")
        .output()
        .expect("second `scswarm serve` must start");
    assert_eq!(second.status.code(), Some(2), "second `serve` must exit 2");
    let why = String::from_utf8_lossy(&second.stderr);
    assert!(why.contains("another `scswarm serve"), "{why}");
    swarm.wait(&format!("land row={}", label(row)), from);
    let count = |needle: &str| {
        swarm.lines()[from..]
            .iter()
            .filter(|line| line.starts_with(needle))
            .count()
    };
    let row = label(row);
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
    let landed = swarm.wait(&format!("land row={}", label(ROWS[0])), from);
    swarm.wait(&format!("mirror project={PROJECT} "), landed);
    swarm.wait(&format!("push row={} ", label(ROWS[0])), from);
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
            .find(|line| line.split_whitespace().nth(1) == Some(row))
            .unwrap_or_else(|| panic!("status must list {row}:\n{table}"));
        assert!(
            line.contains(" pushed "),
            "{row} must read pushed:\n{table}"
        );
    }
}

/// Seed row publishes rows `a` and `b`: the track tip holds the file alone, `to` holds
/// both merges in order and no published file. A file with another `to` fails its row
/// and moves nothing.
fn publishes_next_board(swarm: &Swarm) {
    let from = swarm.mark();
    swarm.track("track/p", &seed("feat/p", "feat/p", ":", "commit: a"));
    swarm.wait(&format!("publish row={} ", label("p/a")), from);
    swarm.wait(&format!("push row={} ", label("p/b")), from);
    assert_eq!(
        swarm.on_remote(&["ls-tree", "--name-only", "track/p"]),
        "SWARM.yaml\n"
    );
    assert_eq!(
        swarm.on_remote(&["show", "track/p:SWARM.yaml"]),
        board("feat/p")
    );
    let merges = swarm.on_remote(&["log", "--first-parent", "--format=%s", "-2", "feat/p"]);
    assert_eq!(merges, "Merge swarm/p/b\nMerge swarm/p/a\n");
    swarm.lacks_next("feat/p");

    swarm.track("track/q", &seed("feat/q", "feat/other", ":", "commit: a"));
    swarm.refused("q", "feat/q", &swarm.head());
}

/// Agent turn rewrites the file after its check: the checked text lands, and the
/// reply names the row commit.
fn publishes_through_agent_commit(swarm: &Swarm) {
    let from = swarm.mark();
    swarm.track("track/g", &seed("feat/g", "feat/g", ":", "commit:"));
    swarm.wait(&format!("push row={} ", label("g/b")), from);
    assert_eq!(
        swarm.on_remote(&["show", "track/g:SWARM.yaml"]),
        board("feat/g")
    );
    let subjects = swarm.on_remote(&["log", "--format=%s", "feat/g"]);
    assert!(subjects.lines().any(|line| line == "plan"), "{subjects}");
    swarm.lacks_next("feat/g");
}

/// Valid board behind a symlink: the host reads no such file.
fn refuses_a_symlink(swarm: &Swarm) {
    let next = next_file("feat/s");
    let link = format!("mv {next} real.yaml && ln -s real.yaml {next}");
    swarm.track("track/s", &seed("feat/s", "feat/s", &link, "commit: a"));
    swarm.refused("s", "feat/s", &swarm.head());
}

/// Human push the worker has not fetched: its push reads behind, and the file lands
/// on the human commit.
fn publishes_onto_a_new_tip(swarm: &Swarm) {
    let from = swarm.mark();
    let track = seed("feat/r", "feat/r", "sleep 10", "commit: a");
    swarm.track("track/r", &track);
    swarm.wait(&format!("step row={} step=1", label("r/a")), from);
    swarm.fetch_every(1, FETCH_STILL);
    thread::sleep(CYCLES);
    swarm.commit_on("track/r", "note.txt", "note\n", "e2e: human push");
    let human = swarm.head();
    swarm.wait(&format!("publish row={} ", label("r/a")), from);
    swarm.fetch_every(FETCH_STILL, 1);
    swarm.wait(&format!("push row={} ", label("r/b")), from);
    assert_eq!(swarm.on_remote(&["rev-parse", "track/r^"]).trim(), human);
    assert_eq!(
        swarm.on_remote(&["ls-tree", "--name-only", "track/r"]),
        "SWARM.yaml\n"
    );
}

/// Remote turns the track push down: the row fails and no track moves.
fn fails_when_the_remote_declines(swarm: &Swarm) {
    let from = swarm.mark();
    swarm.track(
        "track/d",
        &seed("feat/d", "feat/d", "sleep 10", "commit: a"),
    );
    let tip = swarm.head();
    swarm.wait(&format!("step row={} step=1", label("d/a")), from);
    let hook = swarm.remote().join("hooks/update");
    write_hook(&hook, "test \"$1\" != refs/heads/track/d\n");
    swarm.refused("d", "feat/d", &tip);
    fs::remove_file(&hook).expect("hook must be removable");
}

/// Conflict on `to` after the publish: the row retries with its land ref, no new
/// session, then reads failed on the new tip.
fn merges_again_after_publish(swarm: &Swarm) {
    let from = swarm.mark();
    let clash = "echo mine > clash.txt && sleep 10";
    swarm.track("track/m", &seed("feat/m", "feat/m", clash, "commit: a"));
    swarm.wait(&format!("step row={} step=1", label("m/a")), from);
    swarm.commit_on("feat/m", "clash.txt", "theirs\n", "e2e: human commit");
    swarm.wait_state("m/a", "failed");
    let row = label("m/a");
    assert_eq!(swarm.count(from, &format!("publish row={row} ")), 1);
    assert_eq!(swarm.count(from, &format!("fail row={row} ")), 2);
    assert_eq!(swarm.count(from, &format!("step row={row} ")), 1);
}

/// Hook kills the worker in the row commit, after the push and before the land ref.
/// The project rests, the carried row replays as the new file writes it, new row follows.
fn replays_after_a_kill(swarm: &Swarm) {
    let from = swarm.mark();
    let hook = home()
        .join("swarm")
        .join(format!("{PROJECT}.git"))
        .join("hooks/pre-commit");
    write_hook(
        &hook,
        &format!(
            "rm -f '{}'\npkill -9 -f 'scswarm work {PROJECT} '\nexit 1\n",
            hook.display()
        ),
    );
    swarm.track("track/k", &seed("feat/k", "feat/k", ":", "commit: a"));
    swarm.wait(&format!("push row={} ", label("k/b")), from);
    let row = label("k/a");
    assert_eq!(swarm.count(from, &format!("publish row={row} ")), 1);
    assert_eq!(swarm.count(from, &format!("land row={row}")), 1);
    assert_eq!(
        swarm.count(from, &format!("assign row={row} ")),
        2,
        "the kill must force one replay"
    );
    let merges = swarm.on_remote(&["log", "--first-parent", "--format=%s", "-2", "feat/k"]);
    assert_eq!(merges, "Merge swarm/k/b\nMerge swarm/k/a\n");
}

/// Human deletes the track during the row: machine fault, and no push brings it back.
fn stops_on_an_absent_track_branch(swarm: &Swarm) {
    let from = swarm.mark();
    swarm.track(
        "track/e",
        &seed("feat/e", "feat/e", "sleep 10", "commit: a"),
    );
    swarm.wait(&format!("step row={} step=1", label("e/a")), from);
    swarm.git(&["push", "--quiet", "origin", "--delete", "track/e"]);
    swarm.wait("track branch `track/e` must stand on the remote", from);
    assert_eq!(swarm.on_remote(&["for-each-ref", "refs/heads/track/e"]), "");
    let subjects = swarm.on_remote(&["log", "--format=%s", "feat/e"]);
    assert!(!subjects.contains("swarm/e/"), "{subjects}");
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
        refuse_live_swarm();
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
        credentials();
        swarm.git(&["checkout", "--quiet", "--orphan", "main"]);
        swarm.write("scswarm.yaml", CONFIG);
        swarm.commit_here(CONTAINERFILE, &containerfile(1), "e2e: project", "main");
        swarm.git(&["checkout", "--quiet", "-b", "side"]);
        swarm.commit_here("side.txt", "side\n", "e2e: side", "side");
        swarm.track("track/a", &track_a());
        swarm.track("track/b", &track_b());
        swarm.second();
        swarm
    }

    fn remote(&self) -> PathBuf {
        scratch().join("remote.git")
    }

    fn second_remote(&self) -> PathBuf {
        scratch().join("second.git")
    }

    /// Second project: own remote, own clone, one track of one row.
    fn second(&self) {
        let work = scratch().join("second");
        let here = |args: &[&str]| succeed(git().arg("-C").arg(&work).args(args), "git");
        let write = |file: &str, text: &str| {
            fs::write(work.join(file), text).expect("scratch file must be writable")
        };
        succeed(
            git()
                .args(["init", "--quiet", "--bare"])
                .arg(self.second_remote()),
            "git",
        );
        succeed(
            git()
                .args(["clone", "--quiet"])
                .arg(self.second_remote())
                .arg(&work),
            "git",
        );
        here(&["checkout", "--quiet", "--orphan", "main"]);
        write("scswarm.yaml", CONFIG);
        write(CONTAINERFILE, &containerfile(1));
        here(&["add", "--all"]);
        here(&["commit", "--quiet", "--message", "e2e: second project"]);
        here(&["push", "--quiet", "origin", "main"]);
        here(&["checkout", "--quiet", "--orphan", "track/c"]);
        here(&["rm", "-r", "--quiet", "--force", "--cached", "."]);
        here(&["clean", "--quiet", "--force", "-d"]);
        write("SWARM.yaml", &track("main", "feat/c", 0, &[ELSEWHERE]));
        here(&["add", "--all"]);
        here(&["commit", "--quiet", "--message", "e2e: track"]);
        here(&["push", "--quiet", "origin", "track/c"]);
    }

    fn host(&self, workers: usize, env: &[&str]) {
        self.write_host(workers, env, &[]);
    }

    fn with_second(&self, workers: usize, env: &[&str]) {
        self.write_host(workers, env, &[(SECOND, self.second_remote())]);
    }

    fn with_dead(&self, workers: usize, env: &[&str]) {
        let dead = scratch().join("dead.git");
        self.write_host(
            workers,
            env,
            &[(SECOND, self.second_remote()), (DEAD, dead)],
        );
    }

    /// First project, then each of `others` by name and remote.
    fn write_host(&self, workers: usize, env: &[&str], others: &[(&str, PathBuf)]) {
        let file = host_file();
        fs::create_dir_all(file.parent().expect("host file has a dir"))
            .expect("config dir must be writable");
        let set = [&[CREDENTIAL.0], env]
            .concat()
            .iter()
            .map(|name| format!("{name}: {name}"))
            .collect::<Vec<_>>()
            .join(", ");
        let entry = |name: &str, remote: &PathBuf| {
            format!(
                "  {name}:\n    remote: {}\n    tracks: track/\n    fetch: 1\n    env:\n      - match: '*'\n        set: {{ {set} }}\n",
                remote.display()
            )
        };
        let mut text = format!(
            "{MARK}\nworkers: {workers}\npoll: 1\nprojects:\n{}",
            entry(PROJECT, &self.remote())
        );
        for (name, remote) in others {
            text.push_str(&entry(name, remote));
        }
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
            .arg("serve")
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
            self.wait(&format!("assign row={} ", label(row)), from);
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

    /// Row `a` of `track` reads failed, the track stays at `tip`, `to` gets no merge.
    fn refused(&self, track: &str, to: &str, tip: &str) {
        self.wait_state(&format!("{track}/a"), "failed");
        let branch = format!("track/{track}");
        assert_eq!(self.on_remote(&["rev-parse", &branch]).trim(), tip);
        let subjects = self.on_remote(&["log", "--format=%s", to]);
        let merge = format!("swarm/{track}/");
        assert!(
            !subjects.contains(&merge),
            "{to} must get no merge:\n{subjects}"
        );
    }

    /// `to` holds no published file.
    fn lacks_next(&self, to: &str) {
        let (next, files) = (
            next_file(to),
            self.on_remote(&["ls-tree", "-r", "--name-only", to]),
        );
        assert!(
            !files.lines().any(|file| file == next),
            "{next} must stay off `{to}`:\n{files}"
        );
    }

    /// Lines since `from` that start with `prefix`.
    fn count(&self, from: usize, prefix: &str) -> usize {
        self.lines()[from..]
            .iter()
            .filter(|line| line.starts_with(prefix))
            .count()
    }

    /// Change the fetch clock of every project in the host file.
    fn fetch_every(&self, old: u32, new: u32) {
        let text = fs::read_to_string(host_file()).expect("host file must be readable");
        let text = text.replace(&format!("fetch: {old}\n"), &format!("fetch: {new}\n"));
        fs::write(host_file(), text).expect("host file must be writable");
    }

    /// Poll `status` until `row` reads `state`.
    fn wait_state(&self, row: &str, state: &str) {
        let start = Instant::now();
        loop {
            let table = succeed(command().args(["status", PROJECT]), "scswarm");
            let reads = |line: &&str| line.split_whitespace().skip(1).take(2).eq([row, state]);
            if table.lines().any(|line| reads(&line)) {
                return;
            }
            assert!(
                start.elapsed() < WAIT,
                "{row} must read {state} within {WAIT:?}:\n{table}"
            );
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
        let filter = format!("--filter=name={SLOT_NAMES}");
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

    /// Commit the scratch clone holds checked out.
    fn head(&self) -> String {
        succeed(
            git().arg("-C").arg(&self.work).args(["rev-parse", "HEAD"]),
            "git",
        )
        .trim()
        .to_string()
    }

    fn commit_here(&self, file: &str, text: &str, message: &str, branch: &str) {
        self.write(file, text);
        self.git(&["add", "--all"]);
        self.git(&["commit", "--quiet", "--message", message]);
        self.git(&["push", "--quiet", "origin", branch]);
    }

    /// Stdout of git in the bare remote.
    fn on_remote(&self, args: &[&str]) -> String {
        succeed(git().arg("-C").arg(self.remote()).args(args), "git")
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
  stub:
    exec: sh -c 'for file in next-*.yaml; do echo broken > $file; done; echo plan' stub
commit: Reply with the message.
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
                "  - id: {id}\n    steps:\n      - run: test \"${}\" = {} && sleep {ROW_SECONDS}\n      - commit: \"{id}\"\n",
                CREDENTIAL.0, CREDENTIAL.1
            )
        })
        .collect();
    format!("from: {from}\nto: {to}\npriority: {priority}\nrows:\n{rows}")
}

/// Track of one row `a` on the stub agent: writes `board(published_to)`, runs `then`,
/// publishes at step `commit`. `from` is the rolled branch: slots hold its image.
fn seed(to: &str, published_to: &str, then: &str, commit: &str) -> String {
    let next = next_file(to);
    let script: String = board(published_to)
        .lines()
        .map(|line| format!("          {line}\n"))
        .collect();
    format!(
        "from: feat/a\nto: {to}\nrows:\n  - id: a\n    agent: stub\n    steps:\n      - run: |\n          set -e\n          cat > {next} <<'EOF'\n{script}          EOF\n          {then}\n      - {commit}\n        publish: {next}\n"
    )
}

/// Published file of one track. A name per track: a podman machine share refuses, for
/// about 1 s, a file the host just deleted, and `git clean` deletes the file of the last row.
fn next_file(to: &str) -> String {
    format!("next-{}.yaml", to.replace('/', "-"))
}

/// Next round: carried row `a` and new row `b` after it.
fn board(to: &str) -> String {
    format!(
        "from: feat/a\nto: {to}\nrows:\n  - id: a\n    steps:\n      - commit: a\n  - id: b\n    after: [a]\n    steps:\n      - commit: b\n"
    )
}

/// Executable hook script at `path`.
fn write_hook(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().expect("hook has a dir")).expect("hook dir must be writable");
    fs::write(path, format!("#!/bin/sh\n{body}")).expect("hook must be writable");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("hook must take a mode");
}

fn containerfile(revision: u32) -> String {
    format!("FROM {BASE}\nENV E2E_REVISION={revision}\n")
}

/// Stop unless the host file is absent or one this test wrote: slots, container
/// names and the host file belong to the user, and `clean` drops them.
fn refuse_live_swarm() {
    let Ok(text) = fs::read_to_string(host_file()) else {
        return;
    };
    assert!(
        text.starts_with(MARK),
        "e2e must run as a user with no swarm: {} holds a live host file",
        host_file().display()
    );
}

/// Every trace of both projects: slots, images, bare repos, logs, host file, scratch.
fn clean() {
    let listing = Command::new("podman")
        .args([
            "ps",
            "--all",
            "--format={{.ID}} {{.ImageID}}",
            &format!("--filter=name={SLOT_NAMES}"),
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
    let _ = fs::remove_dir_all(swarm.join("slot"));
    for project in [PROJECT, SECOND, DEAD] {
        for dir in [
            swarm.join(format!("{project}.git")),
            swarm.join(format!("{project}.git.partial")),
            swarm.join("log").join(project),
            swarm.join("build").join(project),
        ] {
            let _ = fs::remove_dir_all(dir);
        }
    }
    let _ = fs::remove_dir_all(scratch());
    let _ = fs::remove_file(host_file());
    for (name, _) in [CREDENTIAL, EXTRA] {
        let _ = fs::remove_file(credential_dir().join(name));
    }
}

/// Credential files in the shared directory. Made mode 0700 when absent; an
/// existing directory keeps its mode, so a bad mode fails the test.
fn credentials() {
    let dir = credential_dir();
    if !dir.exists() {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)
            .expect("credential dir must be writable");
    }
    for (name, value) in [CREDENTIAL, EXTRA] {
        fs::write(dir.join(name), format!("{value}\n")).expect("credential must be writable");
    }
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
    config().join("host.yaml")
}

/// Row as event lines name it.
fn label(row: &str) -> String {
    format!("{PROJECT}/{row}")
}

fn credential_dir() -> PathBuf {
    config().join("credentials")
}

fn config() -> PathBuf {
    env::var_os("XDG_CONFIG_HOME")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".config"))
        .join("scswarm")
}

fn home() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .expect("HOME must hold the home dir")
}
