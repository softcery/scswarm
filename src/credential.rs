use std::ffi::OsString;
use std::fs;
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::paths;

/// Group and other mode bits: none allowed.
const SHARED_BITS: u32 = 0o077;

/// Name and secret value. No `Debug`.
pub struct Credential {
    pub name: String,
    pub value: OsString,
}

/// Mode, one file per name, no value read. Exit 1.
pub fn check(names: &[String]) -> Result<()> {
    files(names).map(drop).map_err(Error::Invalid)
}

/// Values, one trailing `\n` stripped. Exit 2.
pub fn load(names: &[String]) -> Result<Vec<Credential>> {
    files(names)
        .map_err(Error::Failed)?
        .into_iter()
        .map(|(name, file)| read(name, &file))
        .collect()
}

/// Mode 0700 plus a lookup that works proves owner or root.
fn files(names: &[String]) -> std::result::Result<Vec<(String, PathBuf)>, String> {
    if names.is_empty() {
        return Ok(Vec::new());
    }
    let dir = paths::credentials().map_err(|why| why.to_string())?;
    let shown = dir.display();
    let meta = fs::metadata(&dir)
        .map_err(|why| format!("credential directory {shown} must exist: {why}"))?;
    if !meta.is_dir() || meta.permissions().mode() & SHARED_BITS != 0 {
        return Err(format!(
            "credential directory {shown} must be a directory of mode 0700"
        ));
    }
    names
        .iter()
        .map(|name| {
            let file = dir.join(name);
            match fs::metadata(&file) {
                Ok(meta) if meta.is_file() => Ok((name.clone(), file)),
                _ => Err(format!(
                    "credential {name}, named in the host file field `env`, must be a file in {shown}"
                )),
            }
        })
        .collect()
}

fn read(name: String, file: &Path) -> Result<Credential> {
    let mut bytes = fs::read(file)
        .map_err(|why| Error::Failed(format!("credential file {}: {why}", file.display())))?;
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
    }
    Ok(Credential {
        name,
        value: OsString::from_vec(bytes),
    })
}
