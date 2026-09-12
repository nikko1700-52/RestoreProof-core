//! The open-source edition's promises, enforced by the build.
//!
//! `PREMIUM.md` says that `RestoreProof` Core contains no licence check, no
//! feature flag, no telemetry and no account, and that no capability will be
//! removed from it to create a reason to buy the commercial edition.
//!
//! A promise in a Markdown file decays. These tests make it a build failure, so
//! the boundary between the two editions cannot erode by accident — not through
//! a well-meaning pull request, and not through a future maintainer who never
//! read `PREMIUM.md`.
//!
//! If one of these fails, the question is not "how do I silence it" but "does
//! this belong in the commercial repository instead".

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::{Path, PathBuf};

/// Identifiers that must never appear in the *code* of the open-source edition.
///
/// Documentation may discuss them freely — explaining what the commercial
/// edition does is the point of `PREMIUM.md` — so comments are stripped before
/// scanning.
const FORBIDDEN_IN_CODE: [&str; 12] = [
    "license_key",
    "licence_key",
    "entitlement",
    "subscription",
    "telemetry",
    "phone_home",
    "activation_key",
    "trial_expired",
    "paywall",
    "is_premium",
    "requires_premium",
    "upgrade_prompt",
];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .to_path_buf()
}

/// Every Rust source file in the workspace.
fn rust_sources() -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut stack = vec![workspace_root().join("crates")];
    while let Some(directory) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                files.push(path);
            }
        }
    }
    assert!(!files.is_empty(), "no Rust sources found");
    files
}

/// Remove comments so that documentation discussing the commercial edition does
/// not trip the scan. Not a full parser — deliberately conservative.
fn strip_comments(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut rest = source;

    // Block comments first.
    while let Some(start) = rest.find("/*") {
        out.push_str(rest.get(..start).unwrap_or_default());
        match rest.get(start..).and_then(|tail| tail.find("*/")) {
            Some(end) => rest = rest.get(start + end + 2..).unwrap_or_default(),
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);

    out.lines()
        .map(|line| match line.find("//") {
            Some(index) => line.get(..index).unwrap_or_default(),
            None => line,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn the_open_source_edition_contains_no_commercial_machinery() {
    let mut offenders: Vec<String> = Vec::new();

    for path in rust_sources() {
        // This file necessarily names the things it forbids.
        if path.ends_with("open_source_guarantees.rs") {
            continue;
        }
        let source = std::fs::read_to_string(&path).unwrap_or_default();
        let code = strip_comments(&source).to_lowercase();
        for forbidden in FORBIDDEN_IN_CODE {
            if code.contains(forbidden) {
                offenders.push(format!("{}: `{forbidden}`", path.display()));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "RestoreProof Core must contain no licence, entitlement or telemetry machinery.\n\
         PREMIUM.md promises this, and the promise is load-bearing: the engine that decides \
         whether your disaster recovery works has to be inspectable.\n\
         If this belongs to the commercial edition, it belongs in that repository.\n\nFound:\n  {}",
        offenders.join("\n  ")
    );
}

#[test]
fn no_crate_depends_on_a_commercial_crate() {
    let mut offenders: Vec<String> = Vec::new();
    let mut manifests = vec![workspace_root().join("Cargo.toml")];

    let crates_dir = workspace_root().join("crates");
    if let Ok(entries) = std::fs::read_dir(&crates_dir) {
        for entry in entries.flatten() {
            let manifest = entry.path().join("Cargo.toml");
            if manifest.is_file() {
                manifests.push(manifest);
            }
        }
    }

    for manifest in manifests {
        let text = std::fs::read_to_string(&manifest)
            .unwrap_or_default()
            .to_lowercase();
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('#') {
                continue;
            }
            if line.contains("premium") || line.contains("enterprise") {
                offenders.push(format!("{}: {line}", manifest.display()));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "the open-source edition must build and run entirely on its own.\n\
         A dependency on a commercial crate would make it unbuildable for everyone else.\n\nFound:\n  {}",
        offenders.join("\n  ")
    );
}

#[test]
fn premium_features_are_described_as_absent_from_this_repository() {
    let premium = std::fs::read_to_string(workspace_root().join("PREMIUM.md"))
        .expect("PREMIUM.md must exist: the boundary has to be written down somewhere");
    // Emphasis markers and line wrapping are presentation, not content: a
    // promise must not be able to disappear because someone reflowed a
    // paragraph, and must not survive because they did.
    let premium = premium.replace(['*', '_'], "");
    let premium = premium.split_whitespace().collect::<Vec<_>>().join(" ");

    for expected in [
        "no licence check",
        "no telemetry",
        "implemented in this repository",
    ] {
        assert!(
            premium.contains(expected),
            "PREMIUM.md no longer states `{expected}`.\n\
             Whether the commercial edition exists may change; that none of it is in this \
             repository may not, and PREMIUM.md has to keep saying so."
        );
    }
}

#[test]
fn the_binary_never_asks_for_a_licence_or_an_account() {
    let output = fixtures::cli().arg("--help").assert().success();
    let help = String::from_utf8_lossy(&output.get_output().stdout).to_lowercase();

    for forbidden in [
        "licen",
        "activate",
        "sign in",
        "account",
        "subscription",
        "trial",
    ] {
        assert!(
            !help.contains(forbidden),
            "`restoreproof --help` mentions `{forbidden}`. The open-source edition must be usable \
             with no account and no activation of any kind."
        );
    }
}

mod fixtures;
