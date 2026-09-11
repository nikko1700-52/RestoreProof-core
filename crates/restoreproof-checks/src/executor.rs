//! The check execution loop: dispatch, timeout, retry and reporting.

use std::time::{Duration, Instant};

use async_trait::async_trait;
use chrono::Utc;
use restoreproof_config::checks::{CheckDefaults, CheckKind, CheckSpec};
use restoreproof_core::CheckStatus;
use restoreproof_core::report::{CheckOutcome, MAX_DETAIL_BYTES};

use crate::context::CheckContext;

/// What an executor concluded about one attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckEvaluation {
    /// Outcome of the attempt.
    pub status: CheckStatus,
    /// One-line explanation.
    pub message: String,
    /// Captured evidence: response bodies, command output, query results.
    pub details: Vec<String>,
}

impl CheckEvaluation {
    /// The assertion held.
    #[must_use]
    pub fn passed(message: impl Into<String>) -> Self {
        Self {
            status: CheckStatus::Passed,
            message: message.into(),
            details: Vec::new(),
        }
    }

    /// The check ran and the assertion did not hold.
    #[must_use]
    pub fn failed(message: impl Into<String>) -> Self {
        Self {
            status: CheckStatus::Failed,
            message: message.into(),
            details: Vec::new(),
        }
    }

    /// The check could not be executed at all.
    ///
    /// This is reserved for "`RestoreProof` could not determine anything":
    /// a missing tool, an unavailable probe. A service that is down is a
    /// [`CheckEvaluation::failed`], not an error.
    #[must_use]
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            status: CheckStatus::Error,
            message: message.into(),
            details: Vec::new(),
        }
    }

    /// The check was deliberately not executed.
    #[must_use]
    pub fn skipped(message: impl Into<String>) -> Self {
        Self {
            status: CheckStatus::Skipped,
            message: message.into(),
            details: Vec::new(),
        }
    }

    /// Attach captured evidence.
    #[must_use]
    pub fn with_details(mut self, details: Vec<String>) -> Self {
        self.details = details;
        self
    }

    /// Attach one line of captured evidence.
    #[must_use]
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.details.push(detail.into());
        self
    }
}

/// Something that can evaluate one kind of check.
///
/// Adding a check type means implementing this trait and registering it in
/// [`execute_once`]; no other crate changes.
#[async_trait]
pub trait CheckExecutor: Send + Sync {
    /// Type discriminator, matching the `type:` field in `checks.yaml`.
    fn kind(&self) -> &'static str;

    /// Evaluate the check once.
    async fn execute(&self, spec: &CheckSpec, context: &CheckContext) -> CheckEvaluation;
}

/// Dispatch a single attempt to the right executor.
async fn execute_once(spec: &CheckSpec, context: &CheckContext) -> CheckEvaluation {
    match &spec.kind {
        CheckKind::Http(_) => crate::http::HttpExecutor.execute(spec, context).await,
        CheckKind::Command(_) => crate::command::CommandExecutor.execute(spec, context).await,
        CheckKind::Sql(_) => crate::sql::SqlExecutor.execute(spec, context).await,
        CheckKind::File(_) => crate::file::FileExecutor.execute(spec, context).await,
        CheckKind::Container(_) => {
            crate::container::ContainerExecutor
                .execute(spec, context)
                .await
        }
        CheckKind::Script(_) => crate::script::ScriptExecutor.execute(spec, context).await,
    }
}

/// Run one check, applying its timeout and retry policy.
///
/// The returned [`CheckOutcome`] is already redacted and truncated: it is safe
/// to write straight into a report.
pub async fn run_check(
    spec: &CheckSpec,
    defaults: &CheckDefaults,
    context: &CheckContext,
) -> CheckOutcome {
    let started_at = Utc::now();
    let clock = Instant::now();

    if !spec.enabled {
        return finish(
            spec,
            context,
            started_at,
            clock,
            0,
            &CheckEvaluation::skipped("disabled in checks.yaml"),
        );
    }

    let timeout = Duration::from_secs(spec.timeout(defaults).max(1));
    let retry = spec.retry_policy(defaults);
    let attempts_allowed = retry.attempts.max(1);

    let mut attempts = 0u32;
    let mut evaluation = CheckEvaluation::error("the check never ran");

    while attempts < attempts_allowed {
        attempts += 1;
        evaluation = match tokio::time::timeout(timeout, execute_once(spec, context)).await {
            Ok(result) => result,
            Err(_) => CheckEvaluation::failed(format!(
                "no answer within the {}s timeout of this check",
                timeout.as_secs()
            )),
        };

        if evaluation.status == CheckStatus::Passed || evaluation.status == CheckStatus::Skipped {
            break;
        }
        if attempts < attempts_allowed {
            tokio::time::sleep(Duration::from_secs(retry.delay_seconds)).await;
        }
    }

    finish(spec, context, started_at, clock, attempts, &evaluation)
}

fn finish(
    spec: &CheckSpec,
    context: &CheckContext,
    started_at: chrono::DateTime<Utc>,
    clock: Instant,
    attempts: u32,
    evaluation: &CheckEvaluation,
) -> CheckOutcome {
    let redactor = &context.redactor;
    CheckOutcome {
        id: spec.id.clone(),
        name: spec.display_name().to_owned(),
        description: spec.description.clone(),
        kind: spec.kind.kind().to_owned(),
        required: spec.required,
        status: evaluation.status,
        started_at,
        duration_seconds: round_ms(clock.elapsed().as_secs_f64()),
        message: redactor.redact_truncated(&evaluation.message, MAX_DETAIL_BYTES),
        details: evaluation
            .details
            .iter()
            .map(|detail| redactor.redact_truncated(detail, MAX_DETAIL_BYTES))
            .collect(),
        attempts,
    }
}

fn round_ms(seconds: f64) -> f64 {
    (seconds * 1000.0).round() / 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{context_with, http_spec};
    use restoreproof_config::checks::RetrySpec;

    #[tokio::test]
    async fn a_disabled_check_is_skipped_without_running() {
        let mut spec = http_spec("disabled", "http://127.0.0.1:1/health");
        spec.enabled = false;
        let outcome = run_check(&spec, &CheckDefaults::default(), &context_with()).await;
        assert_eq!(outcome.status, CheckStatus::Skipped);
        assert_eq!(outcome.attempts, 0);
    }

    #[tokio::test]
    async fn a_failing_check_uses_every_attempt() {
        let mut spec = http_spec("down", "http://127.0.0.1:1/health");
        spec.retry = Some(RetrySpec {
            attempts: 3,
            delay_seconds: 0,
        });
        spec.timeout_seconds = Some(2);
        let outcome = run_check(&spec, &CheckDefaults::default(), &context_with()).await;
        assert_eq!(outcome.status, CheckStatus::Failed);
        assert_eq!(outcome.attempts, 3);
    }

    #[tokio::test]
    async fn outcomes_are_redacted() {
        let mut context = context_with();
        context.redactor.add_secret("topsecretvalue");
        let spec = http_spec("x", "http://127.0.0.1:1/health");
        let outcome = run_check(&spec, &CheckDefaults::default(), &context).await;
        assert!(!outcome.message.contains("topsecretvalue"));
    }
}
