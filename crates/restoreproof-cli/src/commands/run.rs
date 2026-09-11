//! `restoreproof run` and `restoreproof check`.

use std::path::PathBuf;

use restoreproof_core::ExitCode;
use restoreproof_report::terminal::Style;
use restoreproof_runner::{RunMode, RunOptions};

use crate::cli::{GlobalArgs, OutputFormat};
use crate::commands::{load_scenario, tool_info};
use crate::output::Output;

/// Execute a drill and render its report.
pub async fn run(
    global: &GlobalArgs,
    out: &Output,
    mode: RunMode,
    restore_dir: Option<PathBuf>,
) -> ExitCode {
    let scenario = match load_scenario(global, out) {
        Ok(scenario) => scenario,
        Err(code) => return code,
    };

    for warning in &scenario.warnings {
        out.warn(warning);
    }

    if global.no_cleanup || global.keep_environment {
        out.warn(
            "the recovery environment and the restored data will be left in place. They hold a \
             copy of whatever was in the backup: remove them when you are done.",
        );
    }

    let options = RunOptions {
        mode,
        keep_environment: global.keep_environment,
        cleanup_override: global.no_cleanup.then_some(false),
        timeout_override: global.timeout,
        dry_run: global.dry_run,
        restore_dir,
    };

    let outcome = match restoreproof_runner::execute(&scenario, &options, &tool_info()).await {
        Ok(outcome) => outcome,
        Err(err) => {
            out.error(&err.to_string());
            return err.exit_code();
        }
    };

    let rendered = match global.format {
        OutputFormat::Terminal => {
            restoreproof_report::terminal::render(&outcome.report, Style::new(out.color()))
        }
        OutputFormat::Json => {
            match restoreproof_report::render(&outcome.report, restoreproof_report::Format::Json) {
                Ok(text) => text,
                Err(err) => {
                    out.error(&err.to_string());
                    return ExitCode::Internal;
                }
            }
        }
        OutputFormat::Markdown => restoreproof_report::markdown::render(&outcome.report),
    };

    if out.emit(&rendered, global.output.as_deref()).is_err() {
        return ExitCode::Internal;
    }

    if global.output.is_none() && global.format == OutputFormat::Terminal {
        for path in &outcome.report_files {
            out.line(&format!("  Report   {}", path.display()));
        }
        out.line("");
    }

    // The rendered report may have gone to a file or to standard output. A
    // failure must still be visible on standard error, so that a script that
    // redirects the report somewhere else does not lose the reason.
    if outcome.exit_code != ExitCode::Success {
        for error in &outcome.report.errors {
            out.error(error);
        }
        out.error(&format!(
            "drill finished with status {} (exit code {})",
            outcome.report.run.status,
            outcome.exit_code.code()
        ));
    }

    if let Some(project) = &outcome.kept_environment {
        out.warn(&format!(
            "the recovery environment is still running. Remove it with `docker compose -p {project} down -v`."
        ));
    }
    if let Some(workspace) = &outcome.kept_workspace {
        out.warn(&format!(
            "restored data was left in `{}`. Remove it with `rm -rf {}`.",
            workspace.display(),
            workspace.display()
        ));
    }

    outcome.exit_code
}
