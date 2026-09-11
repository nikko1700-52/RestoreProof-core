//! Writing reports to disk and reading them back.

use std::path::{Path, PathBuf};

use restoreproof_core::report::Report;

use crate::error::{ReportError, Result};

/// Output format of a written report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// Machine-readable, and the only format that can be verified.
    Json,
    /// Human-readable.
    Markdown,
}

impl Format {
    /// File extension.
    #[must_use]
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Markdown => "md",
        }
    }
}

/// Render a report to a string in the requested format.
///
/// # Errors
///
/// Returns [`ReportError::Encode`] if the report cannot be serialized.
pub fn render(report: &Report, format: Format) -> Result<String> {
    match format {
        Format::Json => Ok(format!(
            "{}\n",
            serde_json::to_string_pretty(report).map_err(ReportError::Encode)?
        )),
        Format::Markdown => Ok(crate::markdown::render(report)),
    }
}

/// Write a report into `directory`, one file per format.
///
/// The directory is created if needed. On Unix it is created with mode `0700`,
/// because a report describes the contents of a production backup and should
/// not be world-readable by default.
///
/// # Errors
///
/// Returns [`ReportError::Io`] when the directory or a file cannot be written.
pub fn write_all(report: &Report, directory: &Path, formats: &[Format]) -> Result<Vec<PathBuf>> {
    create_private_dir(directory)?;

    let stem = file_stem(report);
    let mut written = Vec::with_capacity(formats.len());
    for format in formats {
        let path = directory.join(format!("{stem}.{}", format.extension()));
        let contents = render(report, *format)?;
        std::fs::write(&path, contents).map_err(|source| ReportError::Io {
            path: path.clone(),
            source,
        })?;
        restrict_file(&path);
        written.push(path);
    }
    Ok(written)
}

/// Base file name for a report: sortable, unique, and safe on every filesystem.
fn file_stem(report: &Report) -> String {
    let project: String = report
        .run
        .project
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let short_id: String = report.run.id.chars().take(8).collect();
    format!(
        "{}-{}-{short_id}",
        report.run.started_at.format("%Y%m%dT%H%M%SZ"),
        project
    )
}

fn create_private_dir(directory: &Path) -> Result<()> {
    if directory.is_dir() {
        return Ok(());
    }
    std::fs::create_dir_all(directory).map_err(|source| ReportError::Io {
        path: directory.to_path_buf(),
        source,
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700));
    }
    Ok(())
}

fn restrict_file(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

/// Read a JSON report back from disk.
///
/// # Errors
///
/// Returns [`ReportError::Io`] or [`ReportError::Decode`].
pub fn load(path: &Path) -> Result<Report> {
    let text = std::fs::read_to_string(path).map_err(|source| ReportError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    serde_json::from_str(&text).map_err(|source| ReportError::Decode {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::sample_report;
    use tempfile::TempDir;

    #[test]
    fn both_formats_are_written_and_json_round_trips() {
        let dir = TempDir::new().unwrap();
        let report = sample_report();
        let written = write_all(&report, dir.path(), &[Format::Json, Format::Markdown]).unwrap();

        assert_eq!(written.len(), 2);
        assert!(
            written
                .iter()
                .any(|p| p.extension().is_some_and(|e| e == "json"))
        );
        assert!(
            written
                .iter()
                .any(|p| p.extension().is_some_and(|e| e == "md"))
        );

        let json_path = written
            .iter()
            .find(|p| p.extension().is_some_and(|e| e == "json"))
            .unwrap();
        let reloaded = load(json_path).unwrap();
        assert_eq!(reloaded, report);
        assert!(reloaded.verify_integrity().unwrap());
    }

    #[test]
    fn the_report_directory_is_created_and_is_private() {
        let dir = TempDir::new().unwrap();
        let nested = dir.path().join("reports/2026");
        write_all(&sample_report(), &nested, &[Format::Json]).unwrap();
        assert!(nested.is_dir());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&nested).unwrap().permissions().mode();
            assert_eq!(
                mode & 0o077,
                0,
                "the report directory must not be readable by others"
            );
        }
    }

    #[test]
    fn file_names_are_sortable_and_sanitised() {
        let mut report = sample_report();
        report.run.project = "weird/../name".to_owned();
        let stem = file_stem(&report);
        assert!(!stem.contains('/'));
        assert!(stem.starts_with("2026"));
    }

    #[test]
    fn a_corrupt_report_is_reported_not_ignored() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("bad.json");
        std::fs::write(&path, b"{ not json").unwrap();
        assert!(matches!(load(&path), Err(ReportError::Decode { .. })));
    }
}
