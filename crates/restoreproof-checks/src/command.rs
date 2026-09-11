//! `type: command` checks.

use async_trait::async_trait;
use restoreproof_config::checks::{CheckKind, CheckSpec};

use crate::context::CheckContext;
use crate::executor::{CheckEvaluation, CheckExecutor};
use crate::program::{ProgramRequest, run};

/// Executes `type: command` checks.
pub struct CommandExecutor;

#[async_trait]
impl CheckExecutor for CommandExecutor {
    fn kind(&self) -> &'static str {
        "command"
    }

    async fn execute(&self, spec: &CheckSpec, context: &CheckContext) -> CheckEvaluation {
        let CheckKind::Command(command) = &spec.kind else {
            return CheckEvaluation::error("internal: check kind mismatch");
        };

        let Some(program) = command.command.first() else {
            return CheckEvaluation::error("no program to run");
        };
        let args = command.command.get(1..).unwrap_or_default();

        // A program given as a path is resolved inside the project; a bare name
        // is looked up on PATH by the operating system.
        let resolved = if program.contains('/') {
            context.project_root.join(program)
        } else {
            std::path::PathBuf::from(program)
        };

        run(
            ProgramRequest {
                program: resolved,
                args,
                workdir: command.workdir.as_deref(),
                environment: &command.environment,
                expected_exit_code: command.expected_exit_code,
                expect_stdout_contains: command.expect_stdout_contains.as_deref(),
                label: format!("`{program}`"),
            },
            context,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{command_spec, context_with};
    use restoreproof_core::CheckStatus;

    #[tokio::test]
    async fn a_successful_command_passes() {
        let spec = command_spec("true", vec!["/bin/true".to_owned()]);
        let evaluation = CommandExecutor.execute(&spec, &context_with()).await;
        assert_eq!(evaluation.status, CheckStatus::Passed);
    }

    #[tokio::test]
    async fn a_failing_command_fails_with_its_exit_code() {
        let spec = command_spec("false", vec!["/bin/false".to_owned()]);
        let evaluation = CommandExecutor.execute(&spec, &context_with()).await;
        assert_eq!(evaluation.status, CheckStatus::Failed);
        assert!(evaluation.message.contains("exited with code 1"));
    }

    #[tokio::test]
    async fn arguments_are_never_interpreted_by_a_shell() {
        let spec = command_spec(
            "echo",
            vec![
                "/bin/echo".to_owned(),
                "$(id)".to_owned(),
                "; rm -rf /".to_owned(),
            ],
        );
        let evaluation = CommandExecutor.execute(&spec, &context_with()).await;
        assert_eq!(evaluation.status, CheckStatus::Passed);
        assert!(
            evaluation.details.iter().any(|d| d.contains("$(id)")),
            "the argument should have been printed literally: {:?}",
            evaluation.details
        );
    }

    #[tokio::test]
    async fn the_injected_environment_is_available_and_the_parent_one_is_not() {
        let spec = command_spec("env", vec!["/usr/bin/env".to_owned()]);
        let evaluation = CommandExecutor.execute(&spec, &context_with()).await;
        let joined = evaluation.details.join("\n");
        assert!(joined.contains("RESTOREPROOF_RESTORE_DIR="));
        assert!(!joined.contains("CARGO_PKG_NAME="));
    }

    #[tokio::test]
    async fn checks_are_skipped_when_command_execution_is_disabled() {
        let mut context = context_with();
        context.security.allow_command_checks = false;
        let spec = command_spec("true", vec!["/bin/true".to_owned()]);
        let evaluation = CommandExecutor.execute(&spec, &context).await;
        assert_eq!(evaluation.status, CheckStatus::Skipped);
    }

    #[tokio::test]
    async fn a_missing_program_is_an_error() {
        let spec = command_spec("missing", vec!["definitely-not-installed-xyz".to_owned()]);
        let evaluation = CommandExecutor.execute(&spec, &context_with()).await;
        assert_eq!(evaluation.status, CheckStatus::Error);
    }
}
