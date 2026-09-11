//! A local directory used as a backup source.
//!
//! This is the source that makes the open-source edition usable without any
//! extra tooling: point it at the directory your dump job writes to and the
//! drill restores from it.

use std::path::{Path, PathBuf};
use std::time::Instant;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::error::{Result, StorageError};
use crate::metadata::{BackupMetadata, RestoreOutcome};
use crate::source::{BackupSource, StorageLimits, directory_size};

/// Optional sidecar describing when the backup was taken.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LocalMetadataFile {
    /// RFC 3339 timestamp of the backup.
    created_at: DateTime<Utc>,
    /// Optional identifier recorded by the backup job.
    #[serde(default)]
    snapshot_id: Option<String>,
}

/// A directory (or single file) on the local filesystem.
#[derive(Debug, Clone)]
pub struct LocalSource {
    path: PathBuf,
    metadata_file: Option<PathBuf>,
    #[allow(dead_code)]
    limits: StorageLimits,
}

impl LocalSource {
    /// Build a source for an already resolved and confined path.
    #[must_use]
    pub const fn new(path: PathBuf, metadata_file: Option<PathBuf>, limits: StorageLimits) -> Self {
        Self {
            path,
            metadata_file,
            limits,
        }
    }

    /// Most recent modification time under the source path.
    fn newest_modification(&self) -> Option<DateTime<Utc>> {
        let mut newest: Option<DateTime<Utc>> = None;
        let mut stack = vec![self.path.clone()];
        while let Some(current) = stack.pop() {
            let Ok(metadata) = std::fs::symlink_metadata(&current) else {
                continue;
            };
            if metadata.is_dir() {
                if let Ok(entries) = std::fs::read_dir(&current) {
                    for entry in entries.flatten() {
                        stack.push(entry.path());
                    }
                }
            }
            if let Ok(modified) = metadata.modified() {
                let stamp: DateTime<Utc> = modified.into();
                newest = Some(newest.map_or(stamp, |current| current.max(stamp)));
            }
        }
        newest
    }
}

#[async_trait]
impl BackupSource for LocalSource {
    fn source_type(&self) -> &'static str {
        "local"
    }

    fn location(&self) -> String {
        self.path.display().to_string()
    }

    fn required_tool(&self) -> Option<&'static str> {
        None
    }

    async fn inspect(&self) -> Result<BackupMetadata> {
        if !self.path.exists() {
            return Err(StorageError::InvalidBackup(format!(
                "`{}` does not exist",
                self.path.display()
            )));
        }

        let mut notes = Vec::new();
        let mut snapshot_id = None;
        let created_at = match &self.metadata_file {
            Some(path) => {
                let text = std::fs::read_to_string(path).map_err(|source| StorageError::Io {
                    operation: "reading the backup metadata file",
                    path: path.clone(),
                    source,
                })?;
                let parsed: LocalMetadataFile =
                    serde_json::from_str(&text).map_err(|err| StorageError::InvalidBackup(format!(
                        "`{}` is not a valid metadata file: {err}. Expected \
                         {{\"created_at\": \"<RFC 3339>\"}}.",
                        path.display()
                    )))?;
                snapshot_id = parsed.snapshot_id;
                Some(parsed.created_at)
            }
            None => {
                let derived = self.newest_modification();
                if derived.is_some() {
                    notes.push(
                        "the backup timestamp was derived from file modification times, not from \
                         backup metadata; it is an approximation"
                            .to_owned(),
                    );
                }
                derived
            }
        };

        let size_bytes = if self.path.is_dir() {
            Some(directory_size(&self.path)?)
        } else {
            std::fs::metadata(&self.path).ok().map(|m| m.len())
        };

        Ok(BackupMetadata {
            source_type: "local".to_owned(),
            location: self.location(),
            snapshot_id,
            created_at,
            size_bytes,
            notes,
        })
    }

    async fn restore(&self, destination: &Path) -> Result<RestoreOutcome> {
        let started = Instant::now();
        let source = self.path.clone();
        let target = destination.to_path_buf();

        // Copying is blocking work; keep it off the async reactor.
        let (bytes, log) = tokio::task::spawn_blocking(move || copy_tree(&source, &target))
            .await
            .map_err(|err| StorageError::InvalidBackup(format!("restore task failed: {err}")))??;

        Ok(RestoreOutcome {
            destination: destination.to_path_buf(),
            bytes_restored: Some(bytes),
            duration: started.elapsed(),
            log,
        })
    }
}

/// Copy a tree, refusing anything that is not a regular file or a directory.
///
/// Symbolic links, sockets, FIFOs and device nodes are skipped and reported.
/// Recreating them would let a crafted backup place a link inside the recovery
/// environment that points at the host filesystem.
fn copy_tree(source: &Path, destination: &Path) -> Result<(u64, Vec<String>)> {
    let mut copied = 0u64;
    let mut log: Vec<String> = Vec::new();
    let mut skipped = 0usize;

    let metadata = std::fs::symlink_metadata(source).map_err(|err| StorageError::Io {
        operation: "reading metadata",
        path: source.to_path_buf(),
        source: err,
    })?;

    if metadata.is_file() {
        let name = source.file_name().unwrap_or_else(|| std::ffi::OsStr::new("backup"));
        let target = destination.join(name);
        copied = copy_file(source, &target)?;
        log.push(format!("copied 1 file ({copied} bytes)"));
        return Ok((copied, log));
    }

    if !metadata.is_dir() {
        return Err(StorageError::InvalidBackup(format!(
            "`{}` is neither a regular file nor a directory",
            source.display()
        )));
    }

    let mut files = 0usize;
    let mut stack = vec![PathBuf::new()];
    while let Some(relative) = stack.pop() {
        let current = source.join(&relative);
        let entries = std::fs::read_dir(&current).map_err(|err| StorageError::Io {
            operation: "listing",
            path: current.clone(),
            source: err,
        })?;

        let target_dir = destination.join(&relative);
        std::fs::create_dir_all(&target_dir).map_err(|err| StorageError::Io {
            operation: "creating directory",
            path: target_dir.clone(),
            source: err,
        })?;

        for entry in entries.flatten() {
            let entry_metadata = match entry.metadata() {
                Ok(value) => value,
                Err(_) => continue,
            };
            let child = relative.join(entry.file_name());
            if entry_metadata.is_dir() {
                stack.push(child);
            } else if entry_metadata.is_file() {
                copied = copied.saturating_add(copy_file(&source.join(&child), &destination.join(&child))?);
                files += 1;
            } else {
                skipped += 1;
                if skipped <= 10 {
                    log.push(format!(
                        "skipped `{}`: only regular files and directories are restored",
                        child.display()
                    ));
                }
            }
        }
    }

    log.push(format!("copied {files} file(s) ({copied} bytes)"));
    if skipped > 0 {
        log.push(format!(
            "{skipped} entry/entries were skipped because they are not regular files"
        ));
    }
    Ok((copied, log))
}

fn copy_file(source: &Path, destination: &Path) -> Result<u64> {
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent).map_err(|err| StorageError::Io {
            operation: "creating directory",
            path: parent.to_path_buf(),
            source: err,
        })?;
    }
    std::fs::copy(source, destination).map_err(|err| StorageError::Io {
        operation: "copying",
        path: source.to_path_buf(),
        source: err,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn source_dir() -> TempDir {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("pgdata")).unwrap();
        fs::write(dir.path().join("dump.sql"), b"CREATE TABLE orders();").unwrap();
        fs::write(dir.path().join("pgdata/base"), vec![7u8; 512]).unwrap();
        dir
    }

    #[tokio::test]
    async fn inspect_reports_size_and_an_approximate_timestamp() {
        let dir = source_dir();
        let source = LocalSource::new(dir.path().to_path_buf(), None, StorageLimits::default());
        let metadata = source.inspect().await.unwrap();
        assert_eq!(metadata.source_type, "local");
        assert_eq!(metadata.size_bytes, Some(512 + 22));
        assert!(metadata.created_at.is_some());
        assert!(metadata.notes.iter().any(|n| n.contains("approximation")));
    }

    #[tokio::test]
    async fn a_metadata_file_provides_an_exact_timestamp() {
        let dir = source_dir();
        let metadata_path = dir.path().join("backup-metadata.json");
        fs::write(
            &metadata_path,
            br#"{"created_at":"2026-01-15T08:00:00Z","snapshot_id":"nightly-42"}"#,
        )
        .unwrap();
        let source = LocalSource::new(
            dir.path().to_path_buf(),
            Some(metadata_path),
            StorageLimits::default(),
        );
        let metadata = source.inspect().await.unwrap();
        assert_eq!(
            metadata.created_at.unwrap().to_rfc3339(),
            "2026-01-15T08:00:00+00:00"
        );
        assert_eq!(metadata.snapshot_id.as_deref(), Some("nightly-42"));
        assert!(metadata.notes.is_empty());
    }

    #[tokio::test]
    async fn a_malformed_metadata_file_is_an_error_not_a_guess() {
        let dir = source_dir();
        let metadata_path = dir.path().join("backup-metadata.json");
        fs::write(&metadata_path, b"not json").unwrap();
        let source = LocalSource::new(
            dir.path().to_path_buf(),
            Some(metadata_path),
            StorageLimits::default(),
        );
        let err = source.inspect().await.unwrap_err();
        assert!(err.to_string().contains("valid metadata file"));
    }

    #[tokio::test]
    async fn restore_copies_the_tree() {
        let dir = source_dir();
        let target = TempDir::new().unwrap();
        let source = LocalSource::new(dir.path().to_path_buf(), None, StorageLimits::default());
        let outcome = source.restore(target.path()).await.unwrap();

        assert_eq!(outcome.bytes_restored, Some(512 + 22));
        assert!(target.path().join("dump.sql").is_file());
        assert!(target.path().join("pgdata/base").is_file());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn restore_refuses_to_recreate_symlinks() {
        let dir = source_dir();
        std::os::unix::fs::symlink("/etc/passwd", dir.path().join("escape")).unwrap();
        let target = TempDir::new().unwrap();
        let source = LocalSource::new(dir.path().to_path_buf(), None, StorageLimits::default());
        let outcome = source.restore(target.path()).await.unwrap();

        assert!(!target.path().join("escape").exists());
        assert!(outcome.log.iter().any(|line| line.contains("skipped")));
    }

    #[tokio::test]
    async fn a_missing_source_is_reported() {
        let source = LocalSource::new(
            PathBuf::from("/nonexistent/backup"),
            None,
            StorageLimits::default(),
        );
        assert!(source.inspect().await.is_err());
    }

    #[tokio::test]
    async fn a_single_file_backup_is_supported() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("dump.sql");
        fs::write(&file, b"SELECT 1;").unwrap();
        let target = TempDir::new().unwrap();
        let source = LocalSource::new(file, None, StorageLimits::default());
        let outcome = source.restore(target.path()).await.unwrap();
        assert_eq!(outcome.bytes_restored, Some(9));
        assert!(target.path().join("dump.sql").is_file());
    }
}
