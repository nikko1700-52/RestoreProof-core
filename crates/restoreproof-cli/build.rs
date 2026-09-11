//! Records the git commit the binary was built from, when one is available.
//!
//! A report says which build produced it; without this the `git_commit` field
//! is simply absent, which is preferable to a guess.

use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=RESTOREPROOF_GIT_COMMIT");

    if std::env::var("RESTOREPROOF_GIT_COMMIT").is_ok() {
        return;
    }

    let commit = Command::new("git")
        .args(["rev-parse", "--short=10", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());

    if let Some(commit) = commit {
        println!("cargo:rustc-env=RESTOREPROOF_GIT_COMMIT={commit}");
    }
}
