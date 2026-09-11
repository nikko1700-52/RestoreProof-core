//! Shared execution path for `command` and `script` checks.
//!
//! Both types are the escape hatch of the product: they run arbitrary local
//! code. That is exactly why they share one audited implementation.
//!
//! * the program and its arguments are an `argv` array, never a command line;
//! * the child starts from an empty environment plus `PATH`, `HOME`, `LANG`,
//!   `TZ`, the three injected `RESTOREPROOF_*` variables and whatever the check
//!   declares — a credential in the operator's shell never reaches a check;
//! * stdin is closed, output is capped by `security.max_command_output_bytes`,
//!   and the whole thing is bounded by the check timeout;
//! * the working directory is always inside the project.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use restoreproof_core::CommandSpec;
use restoreproof_core::process::ProcessError;

use crate::context::CheckContext;
use crate::executor::CheckEvaluation;

/// One program execution requested by a check.
pub(crate) struct ProgramRequest<'a> {
    pub(crate) program: PathBuf,
    pub(crate) args: &'a [String],
    pub(crate) workdir: Option<&'a Path>,
    pub(crate) environment: &'a BTreeMap<String, String>,
    pub(crate) expected_exit_code: i32,
    pub(crate) expect_stdout_contains: Option<&'a str>,
    pub(crate) label: String,
}

/// Run a program and turn its result into a check evaluation.
pub(crate) async fn run(request: ProgramRequest<'_>, context: &CheckContext) -> CheckEvaluation {
    if !context.security.allow_command_checks {
        return CheckEvaluation::skipped(
            "command and script checks are disabled by `security.allow_command_checks: false`",
        );
    }

    let workdir = match request.workdir {
        Some(dir) if dir.is_absolute() => dir.to_path_buf(),
        Some(dir) => context.project_root.join(dir),
        None => context.project_root.clone(),
    };

    let mut spec = CommandSpec::new(request.program.as_os_str())
        .args(request.args.iter().map(String::as_str))
        .workdir(&workdir)
        .max_output_bytes(context.security.max_command_output_bytes);

    for (key, value) in context.injected_environment() {
        spec = spec.env(key, value);
    }
    for (key, value) in request.environment {
        spec = spec.env(key.as_str(), value.as_str());
    }

    // The executor loop owns the real deadline; this one is a backstop so a
    // child is always killed even if the loop is cancelled.
    let output = match spec
        .timeout(std::time::Duration::from_secs(3600))
        .run()
        .await
    {
        Ok(output) => output,
        Err(ProcessError::NotFound { program }) => {
            return CheckEvaluation::error(format!(
                "`{program}` was not found. Install it, or use an absolute path inside the project."
            ));
        }
        Err(err) => return CheckEvaluation::error(err.to_string()),
    };

    let mut details = Vec::new();
    if !output.stdout.trim().is_empty() {
        details.push(format!("stdout: {}", output.stdout.trim_end()));
    }
    if !output.stderr.trim().is_empty() {
        details.push(format!("stderr: {}", output.stderr.trim_end()));
    }
    if output.truncated {
        details.push(format!(
            "output was truncated at security.max_command_output_bytes ({} bytes)",
            context.security.max_command_output_bytes
        ));
    }

    let Some(code) = output.status else {
        return CheckEvaluation::failed(format!("{} was terminated by a signal", request.label))
            .with_details(details);
    };

    if code != request.expected_exit_code {
        return CheckEvaluation::failed(format!(
            "{} exited with code {code}, expected {}",
            request.label, request.expected_exit_code
        ))
        .with_details(details);
    }

    if let Some(needle) = request.expect_stdout_contains
        && !output.stdout.contains(needle)
    {
        return CheckEvaluation::failed(format!(
            "{} exited with the expected code but its output does not contain `{needle}`",
            request.label
        ))
        .with_details(details);
    }

    CheckEvaluation::passed(format!("{} exited with code {code}", request.label))
        .with_details(details)
}
