//! Path confinement policy.
//!
//! Every filesystem path that appears in a configuration file goes through
//! [`PathPolicy`]. The rules are:
//!
//! * relative paths are resolved against the directory containing the
//!   configuration file (the *project root*), never against the current working
//!   directory, so that a drill behaves the same from any shell;
//! * paths are canonicalised, which resolves `..` **and symlinks**, before any
//!   decision is taken — a symlink pointing outside the project cannot smuggle
//!   a path past the check;
//! * the canonical result must stay inside the project root, unless the
//!   operator explicitly listed the location in `security.allow_external_paths`;
//! * executable content (script checks) is confined to the project root with no
//!   possible allowlisting: code that runs during a drill must be versioned
//!   alongside the drill.

use std::path::{Component, Path, PathBuf};

use crate::error::{ConfigError, Result};

/// How strict the confinement is for a given field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confinement {
    /// Must stay under the project root. Allowlisting is not accepted.
    ///
    /// Used for anything that is executed or interpreted: scripts, the Compose
    /// file, the checks file.
    ProjectOnly,
    /// Must stay under the project root or under an allowlisted location.
    ///
    /// Used for data: backup repositories, password files, report directories.
    ProjectOrAllowlisted,
}

/// Resolves and confines configuration paths.
#[derive(Debug, Clone)]
pub struct PathPolicy {
    project_root: PathBuf,
    allowed_external: Vec<PathBuf>,
}

impl PathPolicy {
    /// Build a policy rooted at `project_root`.
    ///
    /// # Errors
    ///
    /// Fails if the project root or an allowlisted path cannot be canonicalised
    /// (typically because it does not exist).
    pub fn new(project_root: &Path, allowed_external: &[PathBuf]) -> Result<Self> {
        let project_root = canonicalize(project_root).map_err(|source| ConfigError::Io {
            path: project_root.to_path_buf(),
            source,
        })?;

        let mut resolved = Vec::with_capacity(allowed_external.len());
        for entry in allowed_external {
            if !entry.is_absolute() {
                return Err(ConfigError::Unsafe {
                    reason: format!(
                        "security.allow_external_paths entries must be absolute; `{}` is relative",
                        entry.display()
                    ),
                });
            }
            let canonical = canonicalize(entry).map_err(|source| ConfigError::Io {
                path: entry.clone(),
                source,
            })?;
            if canonical == Path::new("/") {
                return Err(ConfigError::Unsafe {
                    reason: "security.allow_external_paths must not contain `/`".to_owned(),
                });
            }
            resolved.push(canonical);
        }

        Ok(Self {
            project_root,
            allowed_external: resolved,
        })
    }

    /// Canonical project root.
    #[must_use]
    pub fn project_root(&self) -> &Path {
        &self.project_root
    }

    /// Locations explicitly permitted outside the project root.
    #[must_use]
    pub fn allowed_external(&self) -> &[PathBuf] {
        &self.allowed_external
    }

    /// Resolve a path that must already exist.
    ///
    /// # Errors
    ///
    /// Fails if the path is empty, does not exist, or escapes its allowed area.
    pub fn resolve_existing(
        &self,
        field: &str,
        raw: &Path,
        confinement: Confinement,
    ) -> Result<PathBuf> {
        reject_suspicious_literal(field, raw)?;
        let joined = self.join(raw);
        let canonical = canonicalize(&joined).map_err(|_| ConfigError::MissingPath {
            field: field.to_owned(),
            path: joined.clone(),
        })?;
        self.confine(field, canonical, confinement)
    }

    /// Resolve a path that may not exist yet (a report directory, for instance).
    ///
    /// The deepest existing ancestor is canonicalised and confined; the missing
    /// remainder must not contain `..`, so the final location is provably inside
    /// the confined area once created.
    ///
    /// # Errors
    ///
    /// Fails if the path is empty, contains `..` beyond what exists, or escapes
    /// its allowed area.
    pub fn resolve_for_creation(
        &self,
        field: &str,
        raw: &Path,
        confinement: Confinement,
    ) -> Result<PathBuf> {
        reject_suspicious_literal(field, raw)?;
        let joined = self.join(raw);

        let mut existing = joined.as_path();
        let mut remainder: Vec<&std::ffi::OsStr> = Vec::new();
        let canonical_base = loop {
            match canonicalize(existing) {
                Ok(canonical) => break canonical,
                Err(_) => {
                    let Some(name) = existing.file_name() else {
                        return Err(ConfigError::MissingPath {
                            field: field.to_owned(),
                            path: joined.clone(),
                        });
                    };
                    remainder.push(name);
                    let Some(parent) = existing.parent() else {
                        return Err(ConfigError::MissingPath {
                            field: field.to_owned(),
                            path: joined.clone(),
                        });
                    };
                    existing = parent;
                }
            }
        };

        let base = self.confine(field, canonical_base, confinement)?;
        let mut result = base;
        for name in remainder.iter().rev() {
            if *name == std::ffi::OsStr::new("..") || *name == std::ffi::OsStr::new(".") {
                return Err(ConfigError::Unsafe {
                    reason: format!("{field}: `{}` contains a relative component that cannot be resolved safely", raw.display()),
                });
            }
            result.push(name);
        }
        Ok(result)
    }

    /// Join a raw path with the project root when it is relative.
    fn join(&self, raw: &Path) -> PathBuf {
        if raw.is_absolute() {
            raw.to_path_buf()
        } else {
            self.project_root.join(raw)
        }
    }

    /// Enforce the confinement rule on an already canonical path.
    fn confine(&self, field: &str, canonical: PathBuf, confinement: Confinement) -> Result<PathBuf> {
        if canonical.starts_with(&self.project_root) {
            return Ok(canonical);
        }
        match confinement {
            Confinement::ProjectOnly => Err(ConfigError::PathEscapesProject {
                field: field.to_owned(),
                path: canonical,
                root: self.project_root.clone(),
                extra: "\nThis field cannot be allowlisted: executable and orchestration files must \
                        live inside the project."
                    .to_owned(),
            }),
            Confinement::ProjectOrAllowlisted => {
                if self
                    .allowed_external
                    .iter()
                    .any(|allowed| canonical.starts_with(allowed))
                {
                    Ok(canonical)
                } else {
                    Err(ConfigError::PathEscapesProject {
                        field: field.to_owned(),
                        path: canonical,
                        root: self.project_root.clone(),
                        extra: String::new(),
                    })
                }
            }
        }
    }
}

/// Reject path literals that are never legitimate in a configuration file.
fn reject_suspicious_literal(field: &str, raw: &Path) -> Result<()> {
    let display = raw.to_string_lossy();
    if display.trim().is_empty() {
        return Err(ConfigError::Unsafe {
            reason: format!("{field}: path is empty"),
        });
    }
    if display.contains('\0') {
        return Err(ConfigError::Unsafe {
            reason: format!("{field}: path contains a NUL byte"),
        });
    }
    Ok(())
}

/// Canonicalise without following the current working directory.
fn canonicalize(path: &Path) -> std::io::Result<PathBuf> {
    std::fs::canonicalize(path)
}

/// Whether a path contains a `..` component, used by validation messages.
#[must_use]
pub fn has_parent_component(path: &Path) -> bool {
    path.components().any(|c| matches!(c, Component::ParentDir))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    struct Fixture {
        _dir: TempDir,
        root: PathBuf,
        outside: PathBuf,
    }

    fn fixture() -> Fixture {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("project");
        let outside = dir.path().join("outside");
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(root.join("sub/file.txt"), b"hello").unwrap();
        fs::write(outside.join("secret.txt"), b"secret").unwrap();
        Fixture {
            _dir: dir,
            root,
            outside,
        }
    }

    #[test]
    fn relative_paths_resolve_against_the_project_root() {
        let f = fixture();
        let policy = PathPolicy::new(&f.root, &[]).unwrap();
        let resolved = policy
            .resolve_existing("x", Path::new("sub/file.txt"), Confinement::ProjectOnly)
            .unwrap();
        assert!(resolved.ends_with("sub/file.txt"));
        assert!(resolved.starts_with(policy.project_root()));
    }

    #[test]
    fn dot_dot_escape_is_refused() {
        let f = fixture();
        let policy = PathPolicy::new(&f.root, &[]).unwrap();
        let err = policy
            .resolve_existing(
                "backup.repository",
                Path::new("../outside/secret.txt"),
                Confinement::ProjectOrAllowlisted,
            )
            .unwrap_err();
        assert!(matches!(err, ConfigError::PathEscapesProject { .. }));
    }

    #[test]
    fn absolute_escape_is_refused() {
        let f = fixture();
        let policy = PathPolicy::new(&f.root, &[]).unwrap();
        let err = policy
            .resolve_existing(
                "backup.repository",
                &f.outside.join("secret.txt"),
                Confinement::ProjectOrAllowlisted,
            )
            .unwrap_err();
        assert!(matches!(err, ConfigError::PathEscapesProject { .. }));
    }

    #[cfg(unix)]
    #[test]
    fn symlink_escape_is_refused() {
        let f = fixture();
        std::os::unix::fs::symlink(f.outside.join("secret.txt"), f.root.join("link.txt")).unwrap();
        let policy = PathPolicy::new(&f.root, &[]).unwrap();
        let err = policy
            .resolve_existing("checks_file", Path::new("link.txt"), Confinement::ProjectOnly)
            .unwrap_err();
        assert!(
            matches!(err, ConfigError::PathEscapesProject { .. }),
            "a symlink must not be able to smuggle a path out of the project"
        );
    }

    #[test]
    fn allowlisted_locations_are_accepted_for_data() {
        let f = fixture();
        let policy = PathPolicy::new(&f.root, std::slice::from_ref(&f.outside)).unwrap();
        let resolved = policy
            .resolve_existing(
                "backup.repository",
                &f.outside.join("secret.txt"),
                Confinement::ProjectOrAllowlisted,
            )
            .unwrap();
        assert!(resolved.ends_with("secret.txt"));
    }

    #[test]
    fn allowlisting_never_applies_to_executable_fields() {
        let f = fixture();
        let policy = PathPolicy::new(&f.root, std::slice::from_ref(&f.outside)).unwrap();
        let err = policy
            .resolve_existing(
                "checks[0].script",
                &f.outside.join("secret.txt"),
                Confinement::ProjectOnly,
            )
            .unwrap_err();
        assert!(matches!(err, ConfigError::PathEscapesProject { .. }));
    }

    #[test]
    fn missing_paths_are_reported_with_their_field() {
        let f = fixture();
        let policy = PathPolicy::new(&f.root, &[]).unwrap();
        let err = policy
            .resolve_existing("checks_file", Path::new("nope.yaml"), Confinement::ProjectOnly)
            .unwrap_err();
        match err {
            ConfigError::MissingPath { field, .. } => assert_eq!(field, "checks_file"),
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn output_directories_may_not_exist_yet() {
        let f = fixture();
        let policy = PathPolicy::new(&f.root, &[]).unwrap();
        let resolved = policy
            .resolve_for_creation(
                "report.directory",
                Path::new("reports/2026"),
                Confinement::ProjectOrAllowlisted,
            )
            .unwrap();
        assert!(resolved.ends_with("reports/2026"));
        assert!(resolved.starts_with(policy.project_root()));
    }

    #[test]
    fn output_directories_cannot_escape_either() {
        let f = fixture();
        let policy = PathPolicy::new(&f.root, &[]).unwrap();
        let err = policy
            .resolve_for_creation(
                "report.directory",
                Path::new("../outside/reports"),
                Confinement::ProjectOrAllowlisted,
            )
            .unwrap_err();
        assert!(matches!(err, ConfigError::PathEscapesProject { .. }));
    }

    #[test]
    fn empty_paths_are_refused() {
        let f = fixture();
        let policy = PathPolicy::new(&f.root, &[]).unwrap();
        let err = policy
            .resolve_existing("x", Path::new(""), Confinement::ProjectOnly)
            .unwrap_err();
        assert!(matches!(err, ConfigError::Unsafe { .. }));
    }

    #[test]
    fn relative_allowlist_entries_are_refused() {
        let f = fixture();
        let err = PathPolicy::new(&f.root, &[PathBuf::from("relative/path")]).unwrap_err();
        assert!(matches!(err, ConfigError::Unsafe { .. }));
    }

    #[test]
    fn allowlisting_the_filesystem_root_is_refused() {
        let f = fixture();
        let err = PathPolicy::new(&f.root, &[PathBuf::from("/")]).unwrap_err();
        assert!(matches!(err, ConfigError::Unsafe { .. }));
    }

    #[test]
    fn detects_parent_components() {
        assert!(has_parent_component(Path::new("a/../b")));
        assert!(!has_parent_component(Path::new("a/b")));
    }
}
