//! `restoreproof plan`.

use restoreproof_core::ExitCode;
use restoreproof_runner::Plan;

use crate::cli::{GlobalArgs, OutputFormat};
use crate::commands::load_scenario;
use crate::output::Output;

/// Show what a drill would do, without doing it.
pub fn run(global: &GlobalArgs, out: &Output) -> ExitCode {
    let scenario = match load_scenario(global, out) {
        Ok(scenario) => scenario,
        Err(code) => return code,
    };

    let plan = Plan::from_scenario(&scenario);
    let rendered = match global.format {
        OutputFormat::Json => match serde_json::to_string_pretty(&plan) {
            Ok(json) => format!("{json}\n"),
            Err(err) => {
                out.error(&format!("cannot render the plan: {err}"));
                return ExitCode::Internal;
            }
        },
        OutputFormat::Terminal | OutputFormat::Markdown => plan.render_text(),
    };

    if out.emit(&rendered, global.output.as_deref()).is_err() {
        return ExitCode::Internal;
    }
    ExitCode::Success
}
