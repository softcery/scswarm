use std::ffi::OsStr;
use std::io;
use std::path::Path;
use std::process::{Command, Output};

use crate::error::{Error, Result};

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

/// Output goes to operator, not buffer.
pub fn stream<S: AsRef<OsStr>>(bin: &str, dir: &Path, args: &[S]) -> Result<()> {
    let status = command(bin, dir, args)
        .status()
        .map_err(|why| unspawned(bin, args, &why))?;
    if !status.success() {
        return Err(failed(bin, args, &format!("exit {}", code(status.code()))));
    }
    Ok(())
}

fn output<S: AsRef<OsStr>>(bin: &str, dir: &Path, args: &[S]) -> Result<Output> {
    command(bin, dir, args)
        .output()
        .map_err(|why| unspawned(bin, args, &why))
}

fn command<S: AsRef<OsStr>>(bin: &str, dir: &Path, args: &[S]) -> Command {
    let mut command = Command::new(bin);
    command.current_dir(dir).args(args);
    command
}

fn unspawned<S: AsRef<OsStr>>(bin: &str, args: &[S], why: &io::Error) -> Error {
    match why.kind() {
        io::ErrorKind::NotFound => Error::Failed(format!("{bin} must sit on PATH")),
        _ => failed(bin, args, &why.to_string()),
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
    Error::Failed(format!("{bin} {}: {why}", line(args)))
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
