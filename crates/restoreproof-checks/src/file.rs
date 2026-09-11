//! `type: file` checks.
//!
//! These assert facts about what was actually restored: a dump is present, it
//! is not suspiciously small, its digest matches the one recorded when the
//! backup was taken.
//!
//! The path is resolved against the restore directory (or the project) and the
//! result is re-confined after canonicalisation, so a restored symlink cannot
//! be used to read `/etc/shadow` into a report.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use restoreproof_config::checks::{CheckKind, CheckSpec, FileBase, FileCheck};

use crate::context::CheckContext;
use crate::executor::{CheckEvaluation, CheckExecutor};

/// Executes `type: file` checks.
pub struct FileExecutor;

#[async_trait]
impl CheckExecutor for FileExecutor {
    fn kind(&self) -> &'static str {
        "file"
    }

    async fn execute(&self, spec: &CheckSpec, context: &CheckContext) -> CheckEvaluation {
        let CheckKind::File(file) = &spec.kind else {
            return CheckEvaluation::error("internal: check kind mismatch");
        };

        let base = match file.base {
            FileBase::Restore => context.restore_dir.clone(),
            FileBase::Project => context.project_root.clone(),
        };
        let candidate = base.join(&file.path);

        let Some(resolved) = confine(&base, &candidate) else {
            if file.exists {
                return CheckEvaluation::failed(format!(
                    "`{}` is missing from the restored data",
                    file.path.display()
                ));
            }
            return CheckEvaluation::passed(format!(
                "`{}` is absent, as expected",
                file.path.display()
            ));
        };

        if !file.exists {
            return CheckEvaluation::failed(format!(
                "`{}` exists but the check expects it to be absent",
                file.path.display()
            ));
        }

        evaluate_existing(file, &resolved).await
    }
}

/// Canonicalise and require the result to stay under `base`.
fn confine(base: &Path, candidate: &Path) -> Option<PathBuf> {
    let canonical_base = std::fs::canonicalize(base).ok()?;
    let canonical = std::fs::canonicalize(candidate).ok()?;
    canonical.starts_with(&canonical_base).then_some(canonical)
}

async fn evaluate_existing(file: &FileCheck, resolved: &Path) -> CheckEvaluation {
    let metadata = match std::fs::metadata(resolved) {
        Ok(metadata) => metadata,
        Err(err) => {
            return CheckEvaluation::error(format!("cannot read `{}`: {err}", file.path.display()));
        }
    };

    if !metadata.is_file() {
        return CheckEvaluation::failed(format!(
            "`{}` exists but is not a regular file",
            file.path.display()
        ));
    }

    let size = metadata.len();
    let mut details = vec![format!("size: {size} bytes")];

    if let Some(minimum) = file.min_size_bytes
        && size < minimum
    {
        return CheckEvaluation::failed(format!(
            "`{}` is {size} bytes, below the expected minimum of {minimum}",
            file.path.display()
        ))
        .with_details(details);
    }
    if let Some(maximum) = file.max_size_bytes
        && size > maximum
    {
        return CheckEvaluation::failed(format!(
            "`{}` is {size} bytes, above the expected maximum of {maximum}",
            file.path.display()
        ))
        .with_details(details);
    }

    if let Some(expected) = &file.sha256 {
        let path = resolved.to_path_buf();
        let digest = match tokio::task::spawn_blocking(move || sha256_file(&path)).await {
            Ok(Ok(digest)) => digest,
            Ok(Err(err)) => {
                return CheckEvaluation::error(format!(
                    "cannot hash `{}`: {err}",
                    file.path.display()
                ));
            }
            Err(err) => return CheckEvaluation::error(format!("hashing task failed: {err}")),
        };
        details.push(format!("sha256: {digest}"));
        if !digest.eq_ignore_ascii_case(expected) {
            return CheckEvaluation::failed(format!(
                "`{}` has digest {digest}, expected {expected}",
                file.path.display()
            ))
            .with_details(details);
        }
    }

    CheckEvaluation::passed(format!(
        "`{}` is present ({size} bytes)",
        file.path.display()
    ))
    .with_details(details)
}

/// Hash a file without loading it into memory.
fn sha256_file(path: &Path) -> std::io::Result<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read as _;

    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(buffer.get(..read).unwrap_or_default());
    }
    Ok(restoreproof_core::report::hex_encode(&hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{context_in, file_spec};
    use restoreproof_core::CheckStatus;
    use tempfile::TempDir;

    fn setup() -> (TempDir, CheckContext) {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("restore")).unwrap();
        std::fs::write(dir.path().join("restore/dump.sql"), b"SELECT 1;").unwrap();
        let mut context = context_in(&dir);
        context.restore_dir = dir.path().join("restore");
        (dir, context)
    }

    #[tokio::test]
    async fn an_existing_file_passes() {
        let (_dir, context) = setup();
        let spec = file_spec("dump", "dump.sql");
        let evaluation = FileExecutor.execute(&spec, &context).await;
        assert_eq!(evaluation.status, CheckStatus::Passed);
    }

    #[tokio::test]
    async fn a_missing_file_fails() {
        let (_dir, context) = setup();
        let spec = file_spec("missing", "absent.sql");
        let evaluation = FileExecutor.execute(&spec, &context).await;
        assert_eq!(evaluation.status, CheckStatus::Failed);
        assert!(evaluation.message.contains("missing"));
    }

    #[tokio::test]
    async fn a_file_below_the_minimum_size_fails() {
        let (_dir, context) = setup();
        let mut spec = file_spec("dump", "dump.sql");
        if let CheckKind::File(file) = &mut spec.kind {
            file.min_size_bytes = Some(1024);
        }
        let evaluation = FileExecutor.execute(&spec, &context).await;
        assert_eq!(evaluation.status, CheckStatus::Failed);
        assert!(evaluation.message.contains("below the expected minimum"));
    }

    #[test]
    fn hashing_a_file_matches_the_reference_vector() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("abc.txt");
        std::fs::write(&path, b"abc").unwrap();
        assert_eq!(
            sha256_file(&path).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[tokio::test]
    async fn a_matching_digest_passes_and_a_wrong_one_fails() {
        let (_dir, context) = setup();
        let expected = restoreproof_core::report::sha256_hex(b"SELECT 1;");

        let mut spec = file_spec("dump", "dump.sql");
        if let CheckKind::File(file) = &mut spec.kind {
            file.sha256 = Some(expected);
        }
        assert_eq!(
            FileExecutor.execute(&spec, &context).await.status,
            CheckStatus::Passed
        );

        if let CheckKind::File(file) = &mut spec.kind {
            file.sha256 = Some("0".repeat(64));
        }
        assert_eq!(
            FileExecutor.execute(&spec, &context).await.status,
            CheckStatus::Failed
        );
    }

    #[tokio::test]
    async fn a_symlink_escaping_the_restore_directory_is_not_read() {
        let (dir, context) = setup();
        std::os::unix::fs::symlink("/etc/hostname", dir.path().join("restore/escape")).unwrap();
        let spec = file_spec("escape", "escape");
        let evaluation = FileExecutor.execute(&spec, &context).await;
        assert_eq!(evaluation.status, CheckStatus::Failed);
    }

    #[tokio::test]
    async fn an_absence_assertion_passes_when_the_file_is_gone() {
        let (_dir, context) = setup();
        let mut spec = file_spec("gone", "absent.sql");
        if let CheckKind::File(file) = &mut spec.kind {
            file.exists = false;
        }
        assert_eq!(
            FileExecutor.execute(&spec, &context).await.status,
            CheckStatus::Passed
        );
    }
}
