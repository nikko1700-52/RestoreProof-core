//! Terminal output and logging.
//!
//! All user-facing text goes through this module so that `--quiet`,
//! `--output` and colour handling behave identically for every subcommand.

use std::io::{IsTerminal as _, Write as _};
use std::path::Path;

use restoreproof_core::ExitCode;

use crate::cli::GlobalArgs;

/// Where and how the CLI writes.
#[derive(Debug, Clone)]
pub struct Output {
    quiet: bool,
    color: bool,
}

impl Output {
    /// Build from the parsed global options.
    #[must_use]
    pub fn new(global: &GlobalArgs) -> Self {
        Self {
            quiet: global.quiet,
            color: !color_disabled(global) && std::io::stdout().is_terminal(),
        }
    }

    /// Whether ANSI colour should be emitted.
    #[must_use]
    pub const fn color(&self) -> bool {
        self.color
    }

    /// Print a line, unless `--quiet`.
    pub fn line(&self, text: &str) {
        if self.quiet {
            return;
        }
        let mut stdout = std::io::stdout().lock();
        let _ = writeln!(stdout, "{text}");
    }

    /// Print a block of text, unless `--quiet`.
    pub fn block(&self, text: &str) {
        if self.quiet {
            return;
        }
        let mut stdout = std::io::stdout().lock();
        let _ = write!(stdout, "{text}");
        let _ = stdout.flush();
    }

    /// Print an error. Errors are printed even with `--quiet`.
    pub fn error(&self, text: &str) {
        let mut stderr = std::io::stderr().lock();
        let prefix = if self.color {
            "\u{1b}[31merror:\u{1b}[0m"
        } else {
            "error:"
        };
        let _ = writeln!(stderr, "{prefix} {text}");
    }

    /// Print a warning. Warnings are printed even with `--quiet`.
    pub fn warn(&self, text: &str) {
        let mut stderr = std::io::stderr().lock();
        let prefix = if self.color {
            "\u{1b}[33mwarning:\u{1b}[0m"
        } else {
            "warning:"
        };
        let _ = writeln!(stderr, "{prefix} {text}");
    }

    /// Write rendered content to a file, or print it.
    ///
    /// # Errors
    ///
    /// Returns the exit code to use when the file cannot be written.
    pub fn emit(&self, content: &str, destination: Option<&Path>) -> Result<(), ExitCode> {
        match destination {
            Some(path) => {
                std::fs::write(path, content).map_err(|err| {
                    self.error(&format!("cannot write `{}`: {err}", path.display()));
                    ExitCode::Internal
                })?;
                self.line(&format!("Written to {}", path.display()));
                Ok(())
            }
            None => {
                let mut stdout = std::io::stdout().lock();
                let _ = write!(stdout, "{content}");
                let _ = stdout.flush();
                Ok(())
            }
        }
    }
}

/// Whether colour is disabled, by flag or by the `NO_COLOR` convention.
///
/// <https://no-color.org>: any non-empty value disables colour.
fn color_disabled(global: &GlobalArgs) -> bool {
    global.no_color || std::env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty())
}

/// Configure logging from `--verbose` / `--quiet`.
///
/// Logs go to standard error so they never mix with a report written to
/// standard output.
pub fn init_tracing(global: &GlobalArgs) {
    let level = if global.quiet {
        "restoreproof=error"
    } else {
        match global.verbose {
            0 => "restoreproof=warn",
            1 => "restoreproof=info",
            2 => "restoreproof=debug",
            _ => "restoreproof=trace",
        }
    };

    let filter = tracing_subscriber::EnvFilter::try_from_env("RESTOREPROOF_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(level));

    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_target(false)
        .with_ansi(!color_disabled(global) && std::io::stderr().is_terminal())
        .try_init();
}
