//! HTTP checks.
//!
//! Hardening applied on top of the configuration-time guard in
//! `restoreproof_config::http_guard`:
//!
//! * **No proxy.** The client ignores `HTTP_PROXY`/`HTTPS_PROXY`, so a probe
//!   aimed at loopback cannot be quietly routed somewhere else.
//! * **Redirects re-validated.** When redirects are enabled, every hop is
//!   checked against the same loopback rule as the initial URL, and the chain is
//!   bounded.
//! * **Bounded body.** The response is read in chunks up to a ceiling, so a
//!   service answering with an endless stream cannot exhaust memory.
//! * **Secrets from the environment.** Credential headers are read from
//!   environment variables the runner resolved, never from the YAML file.

use async_trait::async_trait;
use reqwest::redirect::Policy;
use restoreproof_config::checks::{CheckKind, CheckSpec, HttpMethod};
use restoreproof_config::http_guard::url_is_loopback;

use crate::context::CheckContext;
use crate::executor::{CheckEvaluation, CheckExecutor};

/// Maximum body kept for assertions and evidence.
const MAX_BODY_BYTES: usize = 64 * 1024;

/// Maximum number of redirects followed when redirects are enabled.
const MAX_REDIRECTS: usize = 5;

/// Connection timeout; the overall timeout is enforced by the executor loop.
const CONNECT_TIMEOUT_SECONDS: u64 = 10;

/// Executes `type: http` checks.
pub struct HttpExecutor;

#[async_trait]
impl CheckExecutor for HttpExecutor {
    fn kind(&self) -> &'static str {
        "http"
    }

    async fn execute(&self, spec: &CheckSpec, context: &CheckContext) -> CheckEvaluation {
        let CheckKind::Http(http) = &spec.kind else {
            return CheckEvaluation::error("internal: check kind mismatch");
        };

        let allow_external = context.security.allow_external_http_targets;
        let redirect = if http.follow_redirects {
            Policy::custom(move |attempt| {
                if attempt.previous().len() >= MAX_REDIRECTS {
                    attempt.error("too many redirects")
                } else if allow_external || url_is_loopback(attempt.url()) {
                    attempt.follow()
                } else {
                    attempt.stop()
                }
            })
        } else {
            Policy::none()
        };

        let client = match reqwest::Client::builder()
            .redirect(redirect)
            .no_proxy()
            .connect_timeout(std::time::Duration::from_secs(CONNECT_TIMEOUT_SECONDS))
            .user_agent(concat!("restoreproof/", env!("CARGO_PKG_VERSION")))
            .build()
        {
            Ok(client) => client,
            Err(err) => {
                return CheckEvaluation::error(format!("cannot build an HTTP client: {err}"));
            }
        };

        let mut request = match http.method {
            HttpMethod::Get => client.get(&http.url),
            HttpMethod::Head => client.head(&http.url),
            HttpMethod::Post => client.post(&http.url),
        };

        for (name, value) in &http.headers {
            request = request.header(name, value);
        }
        for (name, variable) in &http.headers_from_env {
            let Some(secret) = context.secret(variable) else {
                return CheckEvaluation::error(format!(
                    "header `{name}` needs the environment variable `{variable}`, which is not set"
                ));
            };
            request = request.header(name, secret.expose_secret());
        }
        if let Some(body) = &http.body {
            request = request.body(body.clone());
        }

        let response = match request.send().await {
            Ok(response) => response,
            Err(err) => {
                return CheckEvaluation::failed(format!(
                    "{} {} could not be reached: {}",
                    http.method.as_str(),
                    http.url,
                    describe_reqwest_error(&err)
                ));
            }
        };

        let status = response.status().as_u16();
        let final_url = response.url().to_string();
        let (body, truncated) = read_body(response).await;

        let mut details = vec![format!("HTTP {status} from {final_url}")];
        if !body.is_empty() {
            details.push(format!(
                "body{}: {}",
                if truncated { " (truncated)" } else { "" },
                body
            ));
        }

        if status != http.expected_status {
            return CheckEvaluation::failed(format!(
                "expected HTTP {} from {}, got {status}",
                http.expected_status, http.url
            ))
            .with_details(details);
        }

        if let Some(needle) = &http.expect_body_contains
            && !body.contains(needle.as_str())
        {
            return CheckEvaluation::failed(format!(
                "HTTP {status} as expected, but the body does not contain `{needle}`"
            ))
            .with_details(details);
        }

        CheckEvaluation::passed(format!("HTTP {status} from {}", http.url)).with_details(details)
    }
}

/// Read a response body up to [`MAX_BODY_BYTES`].
async fn read_body(mut response: reqwest::Response) -> (String, bool) {
    let mut collected: Vec<u8> = Vec::new();
    let mut truncated = false;
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                let remaining = MAX_BODY_BYTES.saturating_sub(collected.len());
                if remaining == 0 {
                    truncated = true;
                    break;
                }
                if chunk.len() > remaining {
                    collected.extend_from_slice(chunk.get(..remaining).unwrap_or_default());
                    truncated = true;
                    break;
                }
                collected.extend_from_slice(&chunk);
            }
            Ok(None) => break,
            Err(_) => break,
        }
    }
    (String::from_utf8_lossy(&collected).into_owned(), truncated)
}

fn describe_reqwest_error(err: &reqwest::Error) -> String {
    if err.is_connect() {
        "connection refused or host unreachable".to_owned()
    } else if err.is_timeout() {
        "the request timed out".to_owned()
    } else if err.is_redirect() {
        "the redirect chain was refused".to_owned()
    } else {
        err.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{context_with, http_spec};
    use restoreproof_core::CheckStatus;

    #[tokio::test]
    async fn an_unreachable_endpoint_fails_rather_than_errors() {
        // Port 1 on loopback: nothing listens there.
        let spec = http_spec("health", "http://127.0.0.1:1/health");
        let evaluation = HttpExecutor.execute(&spec, &context_with()).await;
        assert_eq!(evaluation.status, CheckStatus::Failed);
        assert!(evaluation.message.contains("could not be reached"));
    }

    #[tokio::test]
    async fn a_missing_env_header_is_an_error_not_a_silent_request() {
        let mut spec = http_spec("health", "http://127.0.0.1:1/health");
        if let CheckKind::Http(http) = &mut spec.kind {
            http.headers_from_env
                .insert("Authorization".to_owned(), "ABSENT_TOKEN".to_owned());
        }
        let evaluation = HttpExecutor.execute(&spec, &context_with()).await;
        assert_eq!(evaluation.status, CheckStatus::Error);
        assert!(evaluation.message.contains("ABSENT_TOKEN"));
    }
}
