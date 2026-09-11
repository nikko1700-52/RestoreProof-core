//! `restoreproof validate`.

use restoreproof_config::Scenario;
use restoreproof_core::ExitCode;

use crate::cli::{GlobalArgs, OutputFormat};
use crate::commands::report_config_error;
use crate::output::Output;

/// Validate the configuration and report every problem at once.
pub fn run(global: &GlobalArgs, out: &Output) -> ExitCode {
    match Scenario::load(&global.config) {
        Ok(scenario) => {
            match global.format {
                OutputFormat::Json => {
                    let document = serde_json::json!({
                        "valid": true,
                        "config": global.config.display().to_string(),
                        "config_sha256": scenario.config_sha256,
                        "project": scenario.raw.project.name,
                        "services": scenario.compose_audit.services,
                        "checks": scenario.checks.checks.len(),
                        "enabled_checks": scenario.enabled_checks().len(),
                        "warnings": scenario.warnings,
                    });
                    if out
                        .emit(&format!("{document:#}\n"), global.output.as_deref())
                        .is_err()
                    {
                        return ExitCode::Internal;
                    }
                }
                OutputFormat::Terminal | OutputFormat::Markdown => {
                    out.line(&format!(
                        "\n  Configuration is valid: {}\n",
                        global.config.display()
                    ));
                    out.line(&format!(
                        "  project   {}\n  services  {}\n  checks    {} ({} enabled, {} required)",
                        scenario.raw.project.name,
                        if scenario.compose_audit.services.is_empty() {
                            "—".to_owned()
                        } else {
                            scenario.compose_audit.services.join(", ")
                        },
                        scenario.checks.checks.len(),
                        scenario.enabled_checks().len(),
                        scenario
                            .enabled_checks()
                            .iter()
                            .filter(|check| check.required)
                            .count()
                    ));
                    if !scenario.warnings.is_empty() {
                        out.line("");
                        for warning in &scenario.warnings {
                            out.warn(warning);
                        }
                    }
                    out.line("");
                }
            }
            ExitCode::Success
        }
        Err(err) => {
            if global.format == OutputFormat::Json {
                let issues = match &err {
                    restoreproof_config::ConfigError::Invalid { issues } => issues
                        .iter()
                        .map(|issue| {
                            serde_json::json!({"field": issue.field, "message": issue.message})
                        })
                        .collect(),
                    other => vec![serde_json::json!({
                        "field": "configuration",
                        "message": other.to_string()
                    })],
                };
                let document = serde_json::json!({
                    "valid": false,
                    "config": global.config.display().to_string(),
                    "issues": issues,
                });
                let _ = out.emit(&format!("{document:#}\n"), global.output.as_deref());
            } else {
                report_config_error(out, &global.config, &err);
            }
            err.exit_code()
        }
    }
}
