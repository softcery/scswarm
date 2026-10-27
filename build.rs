use std::env;
use std::path::Path;
use std::process::Command;

/// Build inputs; any change marks the commit `-dirty`.
const INPUTS: [&str; 4] = ["src", "build.rs", "Cargo.toml", "Cargo.lock"];

/// `SCSWARM_VERSION`: package version, then commit in own checkout.
fn main() {
    let version = env::var("CARGO_PKG_VERSION").expect("cargo sets CARGO_PKG_VERSION");
    let root = env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR");
    let full = match commit(Path::new(&root)) {
        Some(commit) => format!("{version} ({commit})"),
        None => version,
    };
    println!("cargo:rustc-env=SCSWARM_VERSION={full}");
}

/// `None` unless `root` is checkout top: parent repo is not ours.
fn commit(root: &Path) -> Option<String> {
    let top = git(root, &["rev-parse", "--show-toplevel"])?;
    if Path::new(&top).canonicalize().ok()? != root.canonicalize().ok()? {
        return None;
    }
    watch_git(root);
    for input in INPUTS {
        println!("cargo:rerun-if-changed={input}");
    }
    let commit = git(root, &["rev-parse", "--short=10", "HEAD"])?;
    let status = [&["status", "--porcelain", "--"][..], &INPUTS[..]].concat();
    match git(root, &status) {
        Some(_) => Some(format!("{commit}-dirty")),
        None => Some(commit),
    }
}

/// Rerun on checkout, commit, ref pack.
fn watch_git(root: &Path) {
    let mut paths = vec![
        "HEAD".to_string(),
        "packed-refs".to_string(),
        "index".to_string(),
    ];
    paths.extend(git(root, &["symbolic-ref", "-q", "HEAD"]));
    for path in paths {
        let resolved = git(
            root,
            &["rev-parse", "--path-format=absolute", "--git-path", &path],
        );
        if let Some(file) = resolved.filter(|file| Path::new(file).exists()) {
            println!("cargo:rerun-if-changed={file}");
        }
    }
}

/// Trimmed stdout; `None` on failure or empty output.
fn git(root: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()?;
    let text = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (out.status.success() && !text.is_empty()).then_some(text)
}
