use std::fs;
use std::os::unix::fs::FileTypeExt;
use std::path::Path;

use crate::error::{Error, Result};

/// Device nodes behind host `devices` entries, sorted. A GPU node renumbers at
/// boot, so the stamp holds nodes, not entries.
pub fn nodes(entries: &[String]) -> Result<Vec<String>> {
    let mut found = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        collect(Path::new(entry), index + 1, &mut found)?;
    }
    found.sort();
    found.dedup();
    Ok(found)
}

/// Top level: node or folder. Below folder, other files and symlinks drop out.
fn collect(path: &Path, entry: usize, found: &mut Vec<String>) -> Result<()> {
    let kind = fs::symlink_metadata(path)
        .map_err(|why| failed(entry, &format!("must exist on the host: {why}")))?
        .file_type();
    if is_node(kind) {
        return push(path, entry, found);
    }
    if !kind.is_dir() {
        return Err(failed(
            entry,
            "must name a device node or a folder of device nodes",
        ));
    }
    walk(path, entry, found)
}

fn walk(dir: &Path, entry: usize, found: &mut Vec<String>) -> Result<()> {
    let unreadable = |why: std::io::Error| failed(entry, &format!("must be readable: {why}"));
    for child in fs::read_dir(dir).map_err(unreadable)? {
        let child = child.map_err(unreadable)?;
        let kind = child.file_type().map_err(unreadable)?;
        if is_node(kind) {
            push(&child.path(), entry, found)?;
        } else if kind.is_dir() {
            walk(&child.path(), entry, found)?;
        }
    }
    Ok(())
}

fn is_node(kind: fs::FileType) -> bool {
    kind.is_char_device() || kind.is_block_device()
}

fn push(path: &Path, entry: usize, found: &mut Vec<String>) -> Result<()> {
    let text = path
        .to_str()
        .ok_or_else(|| failed(entry, "must name a UTF-8 path"))?;
    found.push(text.to_string());
    Ok(())
}

fn failed(entry: usize, what: &str) -> Error {
    Error::Failed(format!("host field `devices` entry {entry} {what}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_gives_its_nodes() {
        let dir = std::env::temp_dir().join(format!("scswarm-device-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("temp dir");
        fs::write(dir.join("plain"), "").expect("plain file");
        let entries = [dir.to_string_lossy().into_owned(), "/dev/null".to_string()];
        let got = nodes(&entries);
        fs::remove_dir_all(&dir).expect("temp dir removal");
        assert_eq!(got.expect("nodes"), ["/dev/null"]);
    }
}
