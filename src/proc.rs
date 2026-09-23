use std::ffi::OsStr;
use std::io::{self, Read};
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::error::{Error, Result};

/// Interval between deadline checks of a capped child.
const TICK: Duration = Duration::from_millis(100);

/// How a captured child ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    Code(i32),
    /// Signal from outside the host.
    Signal,
    /// Host kill at deadline.
    Timeout,
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

/// Run `command` to its end, or kill it at `timeout`. Output captured, stdin closed.
/// Pipes drain on their own threads, so a chatty child never blocks on a full pipe.
pub fn capture(mut command: Command, timeout: Option<Duration>) -> Result<Captured> {
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
    let exit = wait(&mut child, timeout).map_err(|why| {
        stop(&mut child);
        broke(why)
    })?;
    Ok(Captured {
        exit,
        stdout: collect(stdout).map_err(broke)?,
        stderr: collect(stderr).map_err(broke)?,
    })
}

/// Output goes to operator, not buffer.
pub fn stream<S: AsRef<OsStr>>(bin: &str, dir: &Path, args: &[S]) -> Result<()> {
    let status = command(bin, dir, args)
        .status()
        .map_err(|why| unspawned(bin, &call(bin, args), &why))?;
    if !status.success() {
        return Err(failed(bin, args, &format!("exit {}", code(status.code()))));
    }
    Ok(())
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

/// Deadline passed: kill, reap, report `Timeout`. Deadline past `Instant` range: none.
fn wait(child: &mut Child, timeout: Option<Duration>) -> io::Result<Exit> {
    let Some(deadline) = timeout.and_then(|timeout| Instant::now().checked_add(timeout)) else {
        return child.wait().map(exit);
    };
    while Instant::now() < deadline {
        if let Some(status) = child.try_wait()? {
            return Ok(exit(status));
        }
        thread::sleep(TICK);
    }
    child.kill()?;
    child.wait()?;
    Ok(Exit::Timeout)
}

/// Error path: kill, reap, best effort.
fn stop(child: &mut Child) {
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
    let error = text(&out.stderr).trim().to_string();
    match error.is_empty() {
        true => text(&out.stdout).trim().to_string(),
        false => error,
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

fn code(code: Option<i32>) -> String {
    code.map_or_else(|| "signal".to_string(), |code| code.to_string())
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}
