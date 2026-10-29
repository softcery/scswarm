use std::ffi::OsStr;
use std::io::{self, Read};
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::error::{Error, Result};

/// Interval between exit, deadline and stop checks of a captured child.
const TICK: Duration = Duration::from_millis(100);

/// Asks whether the caller wants the child gone. Called each tick.
pub type Stop<'a> = dyn Fn() -> Result<bool> + 'a;

/// How a captured child ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    Code(i32),
    /// Signal from outside the host.
    Signal,
    /// Host kill at deadline.
    Timeout,
    /// Host kill on `Stop`.
    Stopped,
}

pub struct Captured {
    pub exit: Exit,
    pub stdout: String,
    pub stderr: String,
}

/// Subprocess mechanics: spawn, exit code, error text. Arguments belong to the caller.
pub fn run<S: AsRef<OsStr>>(bin: &str, dir: &Path, args: &[S]) -> Result<String> {
    let out = output(bin, dir, args)?;
    if !out.status.success() {
        return Err(failed(bin, args, &fault(&out)));
    }
    Ok(text(&out.stdout))
}

/// Exit 0 = yes, exit 1 = no, rest = failure.
pub fn probe<S: AsRef<OsStr>>(bin: &str, dir: &Path, args: &[S]) -> Result<bool> {
    let out = output(bin, dir, args)?;
    match out.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(failed(bin, args, &fault(&out))),
    }
}

/// Exit 0 = stdout, exit 1 = none, rest = failure.
pub fn lookup<S: AsRef<OsStr>>(bin: &str, dir: &Path, args: &[S]) -> Result<Option<String>> {
    let out = output(bin, dir, args)?;
    match out.status.code() {
        Some(0) => Ok(Some(text(&out.stdout))),
        Some(1) => Ok(None),
        _ => Err(failed(bin, args, &fault(&out))),
    }
}

/// Run `command` to its end, or kill it at `timeout` or on `stop`. Output captured,
/// stdin closed. Pipes drain on their own threads, so a chatty child never blocks.
pub fn capture(mut command: Command, timeout: Option<Duration>, stop: &Stop) -> Result<Captured> {
    let program = command.get_program().to_string_lossy().into_owned();
    let broke = |why: io::Error| unspawned(&program, &program, &why);
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(broke)?;
    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());
    let exit = wait(&mut child, timeout, stop).map_err(|fault| {
        end(&mut child);
        match fault {
            Fault::Io(why) => broke(why),
            Fault::Stop(why) => why,
        }
    })?;
    Ok(Captured {
        exit,
        stdout: collect(stdout).map_err(broke)?,
        stderr: collect(stderr).map_err(broke)?,
    })
}

/// Sleep `span` in ticks. `true`: `stop` asked first.
pub fn pause(span: Duration, stop: &Stop) -> Result<bool> {
    let Some(deadline) = Instant::now().checked_add(span) else {
        return Ok(false);
    };
    while Instant::now() < deadline {
        if stop()? {
            return Ok(true);
        }
        thread::sleep(TICK.min(deadline.saturating_duration_since(Instant::now())));
    }
    Ok(false)
}

/// Run to the end, output captured. The caller judges the exit.
pub fn answer<S: AsRef<OsStr>>(bin: &str, dir: &Path, args: &[S]) -> Result<Output> {
    output(bin, dir, args)
}

/// Failure of a call `answer` ran, named by argv and the text it printed.
pub fn refused<S: AsRef<OsStr>>(bin: &str, args: &[S], out: &Output) -> Error {
    failed(bin, args, &fault(out))
}

impl Captured {
    /// Last line printed on stderr, else on stdout: the line that names the fault.
    pub fn last_line(&self) -> String {
        spoke(&self.stdout, &self.stderr)
            .lines()
            .last()
            .unwrap_or_default()
            .to_string()
    }
}

fn output<S: AsRef<OsStr>>(bin: &str, dir: &Path, args: &[S]) -> Result<Output> {
    command(bin, dir, args)
        .output()
        .map_err(|why| unspawned(bin, &call(bin, args), &why))
}

fn command<S: AsRef<OsStr>>(bin: &str, dir: &Path, args: &[S]) -> Command {
    let mut command = Command::new(bin);
    command.current_dir(dir).args(args);
    command
}

/// Why `wait` broke off: the child, or the stop check.
enum Fault {
    Io(io::Error),
    Stop(Error),
}

/// Tick until exit, deadline or `stop`; kill and reap on the last two. Deadline
/// past `Instant` range: none.
fn wait(
    child: &mut Child,
    timeout: Option<Duration>,
    stop: &Stop,
) -> std::result::Result<Exit, Fault> {
    let deadline = timeout.and_then(|timeout| Instant::now().checked_add(timeout));
    let ended = loop {
        if let Some(status) = child.try_wait().map_err(Fault::Io)? {
            return Ok(exit(status));
        }
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            break Exit::Timeout;
        }
        if stop().map_err(Fault::Stop)? {
            break Exit::Stopped;
        }
        thread::sleep(TICK);
    };
    child.kill().map_err(Fault::Io)?;
    child.wait().map_err(Fault::Io)?;
    Ok(ended)
}

/// Error path: kill, reap, best effort.
fn end(child: &mut Child) {
    if child.kill().is_ok() {
        let _ = child.wait();
    }
}

fn exit(status: ExitStatus) -> Exit {
    status.code().map_or(Exit::Signal, Exit::Code)
}

fn drain<R: Read + Send + 'static>(pipe: Option<R>) -> JoinHandle<io::Result<Vec<u8>>> {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(mut pipe) = pipe {
            pipe.read_to_end(&mut bytes)?;
        }
        Ok(bytes)
    })
}

fn collect(reader: JoinHandle<io::Result<Vec<u8>>>) -> io::Result<String> {
    let bytes = reader
        .join()
        .map_err(|_| io::Error::other("pipe reader panicked"))??;
    Ok(text(&bytes))
}

/// `context` names the call in the error: argv for host tools, program alone
/// where argv carries agent text.
fn unspawned(bin: &str, context: &str, why: &io::Error) -> Error {
    match why.kind() {
        io::ErrorKind::NotFound => Error::Failed(format!("{bin} must sit on PATH")),
        _ => Error::Failed(format!("{context}: {why}")),
    }
}

/// git states a merge conflict on stdout, a fault on stderr. Take whichever spoke.
fn fault(out: &Output) -> String {
    spoke(&text(&out.stdout), &text(&out.stderr))
}

fn spoke(stdout: &str, stderr: &str) -> String {
    match stderr.trim() {
        "" => stdout.trim().to_string(),
        error => error.to_string(),
    }
}

fn failed<S: AsRef<OsStr>>(bin: &str, args: &[S], why: &str) -> Error {
    Error::Failed(format!("{}: {why}", call(bin, args)))
}

fn call<S: AsRef<OsStr>>(bin: &str, args: &[S]) -> String {
    format!("{bin} {}", line(args))
}

fn line<S: AsRef<OsStr>>(args: &[S]) -> String {
    args.iter()
        .map(|arg| arg.as_ref().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}
