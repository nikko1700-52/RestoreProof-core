//! Subcommand implementations.

pub mod check;
pub mod init;
pub mod plan;
pub mod report;
pub mod run;
pub mod validate;
pub mod version;

use restoreproof_config::{ConfigError, Scenario};
use restoreproof_core::ExitCode;

use crate::cli::GlobalArgs;
use crate::output::Output;

/// Load a scenario, reporting every problem at once.
pub fn load_scenario(global: &GlobalArgs, out: &Output) -> Result<Scenario, ExitCode> {
    match Scenario::load(&global.config) {
        Ok(scenario) => Ok(scenario),
        Err(err) => {
            report_config_error(out, &global.config, &err);
            Err(err.exit_code())
        }
    }
}

/// Print a configuration error with enough context to fix it.
pub fn report_config_error(out: &Output, path: &std::path::Path, err: &ConfigError) {
    if let ConfigError::Io { source, .. } = err
        && source.kind() == std::io::ErrorKind::NotFound
    {
        out.error(&format!(
            "no configuration at `{}`.\nCreate one with `restoreproof init {}`, or point at an \
             existing scenario with `--config`.",
            path.display(),
            path.parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .map_or_else(|| ".".to_owned(), |parent| parent.display().to_string())
        ));
        return;
    }
    out.error(&err.to_string());
}

/// The build information recorded in reports.
#[must_use]
pub fn tool_info() -> restoreproof_core::report::ToolInfo {
    restoreproof_core::report::ToolInfo::detect(
        env!("CARGO_PKG_VERSION"),
        option_env!("RESTOREPROOF_GIT_COMMIT"),
    )
}
