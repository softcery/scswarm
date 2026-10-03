use std::fs::{self, File, TryLockError};
use std::io;
use std::os::fd::BorrowedFd;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::process::Stdio;

use crate::error::{Error, Result};

/// flock on a directory, held while any copy of its handle is open. Sole caller
/// of `File::lock`.
pub struct Hold(File);

/// Wait for `dir` to free, then hold it.
pub fn hold(dir: &Path) -> Result<Hold> {
    let file = open(dir)?;
    file.lock().map_err(|why| failed(dir, &why))?;
    Ok(Hold(file))
}

/// Hold `dir` if free. `None`: held.
pub fn take(dir: &Path) -> Result<Option<Hold>> {
    try_hold(open(dir)?, dir)
}

/// Probe: holds `dir` for microseconds, so a `hold` in flight waits, never fails.
pub fn held(dir: &Path) -> Result<bool> {
    match File::open(dir) {
        Ok(file) => Ok(try_hold(file, dir)?.is_none()),
        Err(why) if why.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(why) => Err(failed(dir, &why)),
    }
}

/// Hold `dir` through `fd`, a handle on it a parent locked: no gap.
pub fn adopt(fd: BorrowedFd, dir: &Path) -> Result<Hold> {
    let file = File::from(fd.try_clone_to_owned().map_err(|why| failed(dir, &why))?);
    let got = file.metadata().map_err(|why| failed(dir, &why))?;
    let want = fs::metadata(dir).map_err(|why| failed(dir, &why))?;
    if (got.dev(), got.ino()) != (want.dev(), want.ino()) {
        return Err(Error::Failed(format!(
            "lock {}: stdin must hold this directory open",
            dir.display()
        )));
    }
    try_hold(file, dir)?
        .ok_or_else(|| Error::Failed(format!("lock {}: another process holds it", dir.display())))
}

impl Hold {
    /// Child stdin. Held until both sides close it.
    pub fn into_stdio(self) -> Stdio {
        Stdio::from(self.0)
    }
}

fn try_hold(file: File, dir: &Path) -> Result<Option<Hold>> {
    match file.try_lock() {
        Ok(()) => Ok(Some(Hold(file))),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(why)) => Err(failed(dir, &why)),
    }
}

fn open(dir: &Path) -> Result<File> {
    File::open(dir).map_err(|why| failed(dir, &why))
}

fn failed(dir: &Path, why: &io::Error) -> Error {
    Error::Failed(format!("lock {}: {why}", dir.display()))
}
