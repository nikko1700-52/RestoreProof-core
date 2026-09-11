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
    /// `JUnit` XML, for a CI system's test reporter.
    Junit,
    /// Prometheus text format, for the `node_exporter` textfile collector.
    Prometheus,
}

impl Format {
    /// File extension.
    #[must_use]
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Markdown => "md",
            Self::Junit => "junit.xml",
            Self::Prometheus => "prom",
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
        Format::Junit => Ok(crate::junit::render(report)),
        Format::Prometheus => Ok(crate::prometheus::render(report)),
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
        write_atomically(&path, &contents)?;
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

/// Write a file so that a reader never sees a half-written report.
///
/// The content goes to a temporary file in the same directory, is flushed and
/// synced, and is then renamed into place — `rename(2)` is atomic within a
/// filesystem. This matters because a Prometheus textfile collector or a CI
/// artefact step can read the directory at any moment, including while a drill
/// is still writing, and a truncated report is worse than a missing one.
fn write_atomically(path: &Path, contents: &str) -> Result<()> {
    use std::io::Write as _;

    let temporary = path.with_extension(format!(
        "{}.tmp",
        path.extension().unwrap_or_default().to_string_lossy()
    ));

    {
        let mut file = std::fs::File::create(&temporary).map_err(|source| ReportError::Io {
            path: temporary.clone(),
            source,
        })?;
        restrict_file(&temporary);
        file.write_all(contents.as_bytes())
            .and_then(|()| file.sync_all())
            .map_err(|source| ReportError::Io {
                path: temporary.clone(),
                source,
            })?;
    }

    std::fs::rename(&temporary, path).map_err(|source| {
        let _ = std::fs::remove_file(&temporary);
        ReportError::Io {
            path: path.to_path_buf(),
            source,
        }
    })?;
    restrict_file(path);
    Ok(())
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
    fn every_format_is_written_and_named_distinctly() {
        let dir = TempDir::new().unwrap();
        let written = write_all(
            &sample_report(),
            dir.path(),
            &[
                Format::Json,
                Format::Markdown,
                Format::Junit,
                Format::Prometheus,
            ],
        )
        .unwrap();

        assert_eq!(written.len(), 4);
        let names: Vec<String> = written
            .iter()
            .filter_map(|path| {
                path.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .collect();
        assert!(
            names.iter().any(|name| name.ends_with(".junit.xml")),
            "{names:?}"
        );
        assert!(
            names.iter().any(|name| name.ends_with(".prom")),
            "{names:?}"
        );

        // No temporary file may survive an atomic write.
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "a temporary file was left behind");
    }

    #[test]
    fn writing_twice_replaces_the_report_cleanly() {
        let dir = TempDir::new().unwrap();
        let report = sample_report();
        write_all(&report, dir.path(), &[Format::Json]).unwrap();
        let written = write_all(&report, dir.path(), &[Format::Json]).unwrap();
        let reloaded = load(&written[0]).unwrap();
        assert_eq!(reloaded, report);
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
