//! The temporary working directory of a drill.
//!
//! Everything a drill writes lives under one directory created outside the
//! project, with mode `0700` on Unix, and removed when the drill ends. Keeping
//! it is opt-in (`--keep-environment`), and the CLI says so loudly, because the
//! directory holds a copy of restored production data.

use std::path::{Path, PathBuf};

use crate::error::{Result, RunnerError};

/// A private temporary directory that cleans itself up.
#[derive(Debug)]
pub struct Workspace {
    root: PathBuf,
    restore_dir: PathBuf,
    keep: bool,
}

impl Workspace {
    /// Create a workspace for a run.
    ///
    /// # Errors
    ///
    /// Returns [`RunnerError::Io`] if the directory cannot be created.
    pub fn create(run_id: &str, keep: bool) -> Result<Self> {
        let root = std::env::temp_dir().join(format!("restoreproof-{run_id}"));
        std::fs::create_dir_all(&root).map_err(|source| RunnerError::Io {
            context: format!("creating the working directory `{}`", root.display()),
            source,
        })?;
        restrict(&root);

        let restore_dir = root.join("restore");
        std::fs::create_dir_all(&restore_dir).map_err(|source| RunnerError::Io {
            context: format!("creating the restore directory `{}`", restore_dir.display()),
            source,
        })?;
        restrict(&restore_dir);

        Ok(Self {
            root,
            restore_dir,
            keep,
        })
    }

    /// Root of the workspace.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Directory the backup is restored into, and which the Compose file mounts
    /// through `${RESTOREPROOF_RESTORE_DIR}`.
    #[must_use]
    pub fn restore_dir(&self) -> &Path {
        &self.restore_dir
    }

    /// Keep the directory instead of removing it on drop.
    pub const fn keep(&mut self, keep: bool) {
        self.keep = keep;
    }

    /// Whether the directory will survive this process.
    #[must_use]
    pub const fn is_kept(&self) -> bool {
        self.keep
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        if self.keep {
            return;
        }
        if let Err(err) = std::fs::remove_dir_all(&self.root) {
            tracing::warn!(
                path = %self.root.display(),
                error = %err,
                "could not remove the temporary working directory"
            );
        }
    }
}

fn restrict(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        if let Err(err) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)) {
            tracing::warn!(path = %path.display(), error = %err, "could not restrict permissions");
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_workspace_is_private_and_removed_on_drop() {
        let root;
        {
            let workspace = Workspace::create("test-private", false).unwrap();
            root = workspace.root().to_path_buf();
            assert!(workspace.restore_dir().is_dir());

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                let mode = std::fs::metadata(&root).unwrap().permissions().mode();
                assert_eq!(mode & 0o077, 0, "restored data must not be world-readable");
            }
        }
        assert!(!root.exists(), "the workspace must be removed on drop");
    }

    #[test]
    fn a_kept_workspace_survives() {
        let root;
        {
            let workspace = Workspace::create("test-kept", true).unwrap();
            root = workspace.root().to_path_buf();
        }
        assert!(root.exists());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
