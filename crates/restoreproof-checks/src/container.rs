//! `type: container` checks.
//!
//! These assert the state of a service in the recovery environment. All
//! container access goes through [`crate::context::EnvironmentProbe`], which is
//! read-only by construction: a check cannot start, stop or exec into a
//! container.

use async_trait::async_trait;
use restoreproof_config::checks::{CheckKind, CheckSpec, ContainerState};

use crate::context::{CheckContext, ProbeError};
use crate::executor::{CheckEvaluation, CheckExecutor};

/// Executes `type: container` checks.
pub struct ContainerExecutor;

#[async_trait]
impl CheckExecutor for ContainerExecutor {
    fn kind(&self) -> &'static str {
        "container"
    }

    async fn execute(&self, spec: &CheckSpec, context: &CheckContext) -> CheckEvaluation {
        let CheckKind::Container(container) = &spec.kind else {
            return CheckEvaluation::error("internal: check kind mismatch");
        };

        let Some(probe) = context.probe.as_ref() else {
            return CheckEvaluation::skipped(
                "no recovery environment is running, so container state cannot be observed",
            );
        };

        let observation = match probe.observe(&container.service).await {
            Ok(observation) => observation,
            Err(ProbeError::UnknownService(service)) => {
                return CheckEvaluation::failed(format!(
                    "service `{service}` is not part of the recovery environment"
                ));
            }
            Err(ProbeError::Unavailable(message)) => {
                return CheckEvaluation::error(format!(
                    "cannot observe service `{}`: {message}",
                    container.service
                ));
            }
        };

        let mut details = vec![format!(
            "state: {}{}",
            observation.state,
            observation
                .health
                .as_ref()
                .map(|health| format!(", health: {health}"))
                .unwrap_or_default()
        )];

        let state_ok = match container.state {
            ContainerState::Running => observation.state == "running",
            ContainerState::Healthy => {
                observation.state == "running" && observation.health.as_deref() == Some("healthy")
            }
            ContainerState::Exited => observation.state == "exited",
        };

        if !state_ok {
            let expected = match container.state {
                ContainerState::Running => "running",
                ContainerState::Healthy => "running and healthy",
                ContainerState::Exited => "exited",
            };
            if container.state == ContainerState::Healthy && observation.health.is_none() {
                details.push(
                    "the image declares no healthcheck, so `state: healthy` can never be satisfied"
                        .to_owned(),
                );
            }
            return CheckEvaluation::failed(format!(
                "service `{}` is `{}`, expected {expected}",
                container.service, observation.state
            ))
            .with_details(details);
        }

        if let Some(expected_code) = container.expected_exit_code {
            match observation.exit_code {
                Some(code) if code == expected_code => {}
                Some(code) => {
                    return CheckEvaluation::failed(format!(
                        "service `{}` exited with code {code}, expected {expected_code}",
                        container.service
                    ))
                    .with_details(details);
                }
                None => {
                    return CheckEvaluation::error(format!(
                        "service `{}` reports no exit code",
                        container.service
                    ))
                    .with_details(details);
                }
            }
        }

        if let Some(port) = container.port {
            match probe.published_port(&container.service, port).await {
                Ok(Some(address)) => match tokio::net::TcpStream::connect(&address).await {
                    Ok(_) => details.push(format!("port {port} reachable at {address}")),
                    Err(err) => {
                        return CheckEvaluation::failed(format!(
                            "service `{}` publishes port {port} at {address} but it refused a \
                             connection: {err}",
                            container.service
                        ))
                        .with_details(details);
                    }
                },
                Ok(None) => {
                    return CheckEvaluation::failed(format!(
                        "service `{}` does not publish port {port}",
                        container.service
                    ))
                    .with_details(details);
                }
                Err(err) => {
                    return CheckEvaluation::error(format!(
                        "cannot resolve the published port of `{}`: {err}",
                        container.service
                    ))
                    .with_details(details);
                }
            }
        }

        CheckEvaluation::passed(format!(
            "service `{}` is {}{}",
            container.service,
            observation.state,
            observation
                .health
                .as_ref()
                .map(|health| format!(" and {health}"))
                .unwrap_or_default()
        ))
        .with_details(details)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::ServiceObservation;
    use crate::test_support::{container_spec, context_with};
    use restoreproof_core::CheckStatus;
    use std::sync::Arc;

    struct FakeProbe(Result<ServiceObservation, String>);

    #[async_trait]
    impl crate::context::EnvironmentProbe for FakeProbe {
        async fn observe(&self, _service: &str) -> Result<ServiceObservation, ProbeError> {
            self.0.clone().map_err(ProbeError::Unavailable)
        }
        async fn published_port(
            &self,
            _service: &str,
            _port: u16,
        ) -> Result<Option<String>, ProbeError> {
            Ok(None)
        }
    }

    fn context_with_probe(observation: ServiceObservation) -> CheckContext {
        let mut context = context_with();
        context.probe = Some(Arc::new(FakeProbe(Ok(observation))));
        context
    }

    fn observation(state: &str, health: Option<&str>) -> ServiceObservation {
        ServiceObservation {
            service: "db".to_owned(),
            state: state.to_owned(),
            health: health.map(str::to_owned),
            exit_code: None,
        }
    }

    #[tokio::test]
    async fn a_running_service_passes() {
        let context = context_with_probe(observation("running", None));
        let spec = container_spec("db", ContainerState::Running);
        assert_eq!(
            ContainerExecutor.execute(&spec, &context).await.status,
            CheckStatus::Passed
        );
    }

    #[tokio::test]
    async fn a_stopped_service_fails() {
        let context = context_with_probe(observation("exited", None));
        let spec = container_spec("db", ContainerState::Running);
        let evaluation = ContainerExecutor.execute(&spec, &context).await;
        assert_eq!(evaluation.status, CheckStatus::Failed);
        assert!(evaluation.message.contains("expected running"));
    }

    #[tokio::test]
    async fn healthy_requires_a_healthcheck_and_says_so() {
        let context = context_with_probe(observation("running", None));
        let spec = container_spec("db", ContainerState::Healthy);
        let evaluation = ContainerExecutor.execute(&spec, &context).await;
        assert_eq!(evaluation.status, CheckStatus::Failed);
        assert!(
            evaluation
                .details
                .iter()
                .any(|d| d.contains("no healthcheck"))
        );
    }

    #[tokio::test]
    async fn a_healthy_service_passes() {
        let context = context_with_probe(observation("running", Some("healthy")));
        let spec = container_spec("db", ContainerState::Healthy);
        assert_eq!(
            ContainerExecutor.execute(&spec, &context).await.status,
            CheckStatus::Passed
        );
    }

    #[tokio::test]
    async fn without_a_running_environment_the_check_is_skipped() {
        let spec = container_spec("db", ContainerState::Running);
        assert_eq!(
            ContainerExecutor
                .execute(&spec, &context_with())
                .await
                .status,
            CheckStatus::Skipped
        );
    }
}
