use std::ffi::OsString;
use std::fs;
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::paths;

/// Group and other mode bits: none allowed.
const SHARED_BITS: u32 = 0o077;
/// Joins variable and file in the worker argument.
const JOIN: char = '=';

/// Container variable and the credential file that feeds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub var: String,
    pub file: String,
}

/// Container variable name and secret value. No `Debug`.
pub struct Credential {
    pub name: String,
    pub value: OsString,
}

/// Mode, one file per binding, no value read. Exit 1.
pub fn check(bindings: &[Binding]) -> Result<()> {
    files(bindings).map(drop).map_err(Error::Invalid)
}

/// Values, one trailing `\n` stripped. Exit 2.
pub fn load(bindings: &[Binding]) -> Result<Vec<Credential>> {
    files(bindings)
        .map_err(Error::Failed)?
        .into_iter()
        .map(|(name, file)| read(name, &file))
        .collect()
}

/// Letters, digits and `_`: a shell variable, a file name with no path.
pub fn plain(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|letter| letter.is_ascii_alphanumeric() || letter == '_')
}

impl TryFrom<String> for Binding {
    type Error = String;

    fn try_from(text: String) -> std::result::Result<Self, String> {
        match text.split_once(JOIN) {
            Some((var, file)) if plain(var) && plain(file) => Ok(Self {
                var: var.to_string(),
                file: file.to_string(),
            }),
            _ => Err(format!(
                "binding must read `VAR{JOIN}FILE`, both letters, digits, `_`"
            )),
        }
    }
}

impl From<Binding> for String {
    fn from(binding: Binding) -> Self {
        format!("{}{JOIN}{}", binding.var, binding.file)
    }
}

/// Mode 0700 plus a lookup that works proves owner or root.
fn files(bindings: &[Binding]) -> std::result::Result<Vec<(String, PathBuf)>, String> {
    if bindings.is_empty() {
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
    bindings
        .iter()
        .map(|binding| {
            let file = dir.join(&binding.file);
            match fs::metadata(&file) {
                Ok(meta) if meta.is_file() => Ok((binding.var.clone(), file)),
                _ => Err(format!(
                    "credential {}, named in the host file field `env`, must be a file in {shown}",
                    binding.file
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
