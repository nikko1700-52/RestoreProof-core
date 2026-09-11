//! The `BackupSource` extension point.

use std::path::Path;
use std::time::Duration;

use async_trait::async_trait;
use restoreproof_config::{ResolvedBackup, SecretSource};
use restoreproof_core::Redactor;

use crate::error::{Result, StorageError};
use crate::metadata::{BackupMetadata, RestoreOutcome};

/// Time and size limits applied to a backup source.
#[derive(Debug, Clone, Copy)]
pub struct StorageLimits {
    /// Ceiling for metadata lookups.
    pub inspect_timeout: Duration,
    /// Ceiling for the restore itself.
    pub restore_timeout: Duration,
    /// Ceiling for captured tool output.
    pub max_output_bytes: usize,
}

impl Default for StorageLimits {
    fn default() -> Self {
        Self {
            inspect_timeout: Duration::from_secs(120),
            restore_timeout: Duration::from_secs(1800),
            max_output_bytes: restoreproof_core::process::DEFAULT_MAX_OUTPUT_BYTES,
        }
    }
}

/// A place a backup can be read from.
///
/// This trait is the extension point for backup storage. The open-source
/// edition ships [`crate::local`], [`crate::restic`] and [`crate::borg`];
/// object stores, hypervisor snapshots and vendor APIs are implemented outside
/// this repository against the same trait, so the runner never changes.
///
/// Implementations must:
///
/// * never write to the repository — a drill is read-only with respect to the
///   backup it verifies;
/// * never print or return a credential, and register any secret they resolve
///   with the [`Redactor`] they are given;
/// * restore only inside `destination`.
#[async_trait]
pub trait BackupSource: Send + Sync {
    /// `local`, `restic`, `borg`, ...
    fn source_type(&self) -> &'static str;

    /// Repository location with credentials removed, for plans and reports.
    fn location(&self) -> String;

    /// External program this source needs, if any.
    fn required_tool(&self) -> Option<&'static str>;

    /// Read metadata about the backup without restoring it.
    ///
    /// # Errors
    ///
    /// See [`StorageError`].
    async fn inspect(&self) -> Result<BackupMetadata>;

    /// Restore the backup into `destination`, which already exists and is empty.
    ///
    /// # Errors
    ///
    /// See [`StorageError`].
    async fn restore(&self, destination: &Path) -> Result<RestoreOutcome>;
}

/// Build the source described by a validated scenario.
///
/// Secrets are resolved here, once, and registered with `redactor` before any
/// external tool runs.
///
/// # Errors
///
/// Returns [`StorageError::Secret`] when a configured secret cannot be read.
pub fn build(
    backup: &ResolvedBackup,
    limits: StorageLimits,
    redactor: &mut Redactor,
) -> Result<Box<dyn BackupSource>> {
    match backup {
        ResolvedBackup::Local {
            path,
            metadata_file,
        } => Ok(Box::new(crate::local::LocalSource::new(
            path.clone(),
            metadata_file.clone(),
            limits,
        ))),
        ResolvedBackup::Restic {
            repository,
            password,
            snapshot,
            include_path,
        } => {
            let secret = resolve_secret(password.as_ref(), redactor)?;
            Ok(Box::new(crate::restic::ResticSource::new(
                repository.clone(),
                secret,
                snapshot.clone(),
                include_path.clone(),
                limits,
            )))
        }
        ResolvedBackup::Borg {
            repository,
            passphrase,
            archive,
        } => {
            let secret = resolve_secret(passphrase.as_ref(), redactor)?;
            Ok(Box::new(crate::borg::BorgSource::new(
                repository.clone(),
                secret,
                archive.clone(),
                limits,
            )))
        }
    }
}

/// How a resolved secret is handed to the backup tool.
///
/// Passing a *path* is preferred over passing the value: the plaintext then
/// never enters this process's memory nor the child's environment.
#[derive(Debug, Clone)]
pub enum ResolvedSecret {
    /// No secret configured; the tool uses its own environment.
    None,
    /// The tool reads the secret from this file.
    File(std::path::PathBuf),
    /// The tool receives the secret through its environment.
    Value(restoreproof_core::Secret),
}

/// Read a configured secret, registering it for redaction when it is a value.
fn resolve_secret(source: Option<&SecretSource>, redactor: &mut Redactor) -> Result<ResolvedSecret> {
    match source {
        None => Ok(ResolvedSecret::None),
        Some(SecretSource::File(path)) => {
            // The file is handed to the tool by path; we still read it once so
            // that its content can be redacted from the tool's own output.
            match std::fs::read_to_string(path) {
                Ok(content) => {
                    redactor.add_secret(content.trim_end_matches(['\n', '\r']));
                    Ok(ResolvedSecret::File(path.clone()))
                }
                Err(err) => Err(StorageError::Secret(format!(
                    "cannot read `{}`: {err}",
                    path.display()
                ))),
            }
        }
        Some(SecretSource::Env(variable)) => match std::env::var(variable) {
            Ok(value) if !value.is_empty() => {
                redactor.add_secret(&value);
                Ok(ResolvedSecret::Value(restoreproof_core::Secret::new(value)))
            }
            Ok(_) => Err(StorageError::Secret(format!(
                "the environment variable `{variable}` is set but empty"
            ))),
            Err(_) => Err(StorageError::Secret(format!(
                "the environment variable `{variable}` is not set"
            ))),
        },
    }
}

/// Total size of a directory tree, not following symlinks.
pub(crate) fn directory_size(root: &Path) -> Result<u64> {
    let mut total = 0u64;
    let mut stack = vec![root.to_path_buf()];
    while let Some(current) = stack.pop() {
        let metadata = std::fs::symlink_metadata(&current).map_err(|source| StorageError::Io {
            operation: "reading metadata",
            path: current.clone(),
            source,
        })?;
        if metadata.is_dir() {
            let entries = std::fs::read_dir(&current).map_err(|source| StorageError::Io {
                operation: "listing",
                path: current.clone(),
                source,
            })?;
            for entry in entries.flatten() {
                stack.push(entry.path());
            }
        } else if metadata.is_file() {
            total = total.saturating_add(metadata.len());
        }
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn directory_size_sums_regular_files_only() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("sub")).unwrap();
        fs::write(dir.path().join("a.bin"), vec![0u8; 100]).unwrap();
        fs::write(dir.path().join("sub/b.bin"), vec![0u8; 50]).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(dir.path().join("a.bin"), dir.path().join("link")).unwrap();

        assert_eq!(directory_size(dir.path()).unwrap(), 150);
    }

    #[test]
    fn a_missing_env_secret_is_reported_clearly() {
        let mut redactor = Redactor::new();
        let err = resolve_secret(
            Some(&SecretSource::Env("RESTOREPROOF_ABSENT_VAR".to_owned())),
            &mut redactor,
        )
        .unwrap_err();
        assert!(err.to_string().contains("is not set"));
    }

    #[test]
    fn a_secret_file_is_registered_for_redaction() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("password");
        fs::write(&path, "super-secret-value\n").unwrap();
        let mut redactor = Redactor::new();
        let resolved = resolve_secret(Some(&SecretSource::File(path.clone())), &mut redactor).unwrap();
        assert!(matches!(resolved, ResolvedSecret::File(_)));
        assert_eq!(redactor.redact("leak: super-secret-value"), "leak: [REDACTED]");
    }

    #[test]
    fn no_secret_is_not_an_error() {
        let mut redactor = Redactor::new();
        assert!(matches!(
            resolve_secret(None, &mut redactor).unwrap(),
            ResolvedSecret::None
        ));
    }
}
