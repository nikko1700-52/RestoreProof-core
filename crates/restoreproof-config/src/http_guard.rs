//! URL and header validation for HTTP checks.
//!
//! An HTTP check exists to probe the environment the drill just started. Left
//! unconstrained it would also be a convenient request-forgery primitive: a
//! configuration file could point it at a cloud metadata endpoint or at an
//! internal service and copy the answer into a report.
//!
//! The rules are therefore:
//!
//! * only `http` and `https`;
//! * no credentials embedded in the URL;
//! * loopback targets only, unless `security.allow_external_http_targets` is
//!   explicitly enabled;
//! * redirects are not followed by default, and when they are, every hop is
//!   validated again by the executor.

use std::net::IpAddr;

use url::{Host, Url};

use crate::error::{ConfigError, Result};

/// Header names that must not be written literally in a configuration file.
const CREDENTIAL_HEADERS: [&str; 4] = [
    "authorization",
    "cookie",
    "proxy-authorization",
    "x-api-key",
];

/// Parse and validate a check URL.
///
/// # Errors
///
/// Returns [`ConfigError::Unsafe`] with an explanation for every rejection.
pub fn validate_url(field: &str, raw: &str, allow_external: bool) -> Result<Url> {
    let url = Url::parse(raw).map_err(|err| ConfigError::Unsafe {
        reason: format!("{field}: `{raw}` is not a valid absolute URL ({err})"),
    })?;

    if !matches!(url.scheme(), "http" | "https") {
        return Err(ConfigError::Unsafe {
            reason: format!(
                "{field}: scheme `{}` is not supported; use http or https",
                url.scheme()
            ),
        });
    }

    if !url.username().is_empty() || url.password().is_some() {
        return Err(ConfigError::Unsafe {
            reason: format!(
                "{field}: the URL embeds credentials. Use `headers_from_env` so the secret stays \
                 out of the configuration file."
            ),
        });
    }

    let Some(host) = url.host() else {
        return Err(ConfigError::Unsafe {
            reason: format!("{field}: the URL has no host"),
        });
    };

    if !allow_external && !is_loopback(&host) {
        return Err(ConfigError::Unsafe {
            reason: format!(
                "{field}: `{host}` is not a loopback address. A recovery drill probes the \
                 environment it just started, so only 127.0.0.0/8, ::1 and `localhost` are allowed \
                 by default. Set `security.allow_external_http_targets: true` if you really need \
                 to reach another host."
            ),
        });
    }

    Ok(url)
}

/// Whether a host designates the local machine.
#[must_use]
pub fn is_loopback(host: &Host<&str>) -> bool {
    match host {
        Host::Domain(name) => {
            let name = name.trim_end_matches('.').to_ascii_lowercase();
            name == "localhost" || name.ends_with(".localhost")
        }
        Host::Ipv4(addr) => IpAddr::V4(*addr).is_loopback(),
        Host::Ipv6(addr) => IpAddr::V6(*addr).is_loopback(),
    }
}

/// Whether a parsed URL targets the local machine.
#[must_use]
pub fn url_is_loopback(url: &Url) -> bool {
    url.host().is_some_and(|host| is_loopback(&host))
}

/// Validate a literal header written in the configuration file.
///
/// # Errors
///
/// Returns [`ConfigError::Unsafe`] for malformed names, non-printable values
/// and credential-carrying headers.
pub fn validate_literal_header(field: &str, name: &str, value: &str) -> Result<()> {
    validate_header_name(field, name)?;
    if CREDENTIAL_HEADERS.contains(&name.to_ascii_lowercase().as_str()) {
        return Err(ConfigError::Unsafe {
            reason: format!(
                "{field}: header `{name}` carries a credential and must not be written in a \
                 configuration file. Use `headers_from_env: {{ {name}: MY_ENV_VAR }}` instead."
            ),
        });
    }
    validate_header_value(field, name, value)
}

/// Validate a header whose value comes from an environment variable.
///
/// # Errors
///
/// Returns [`ConfigError::Unsafe`] for malformed names.
pub fn validate_env_header(field: &str, name: &str, variable: &str) -> Result<()> {
    validate_header_name(field, name)?;
    let valid = !variable.is_empty()
        && variable
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_uppercase() || c == '_')
        && variable
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
    if !valid {
        return Err(ConfigError::Unsafe {
            reason: format!(
                "{field}: `{variable}` is not a valid environment variable name (expected \
                 [A-Z_][A-Z0-9_]*)"
            ),
        });
    }
    Ok(())
}

fn validate_header_name(field: &str, name: &str) -> Result<()> {
    let valid = !name.is_empty()
        && name.len() <= 128
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_".contains(c));
    if valid {
        Ok(())
    } else {
        Err(ConfigError::Unsafe {
            reason: format!("{field}: `{name}` is not a valid HTTP header name"),
        })
    }
}

fn validate_header_value(field: &str, name: &str, value: &str) -> Result<()> {
    if value.len() > 2048 {
        return Err(ConfigError::Unsafe {
            reason: format!("{field}: the value of header `{name}` is too long"),
        });
    }
    if value.chars().any(|c| c.is_control()) {
        return Err(ConfigError::Unsafe {
            reason: format!(
                "{field}: the value of header `{name}` contains a control character, which would \
                 allow header injection"
            ),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_targets_are_accepted() {
        for raw in [
            "http://127.0.0.1:18080/health",
            "http://localhost:8080/",
            "https://[::1]:8443/status",
            "http://127.5.5.5/health",
        ] {
            assert!(
                validate_url("u", raw, false).is_ok(),
                "{raw} should be accepted"
            );
        }
    }

    #[test]
    fn external_targets_are_refused_by_default() {
        let err = validate_url("u", "http://169.254.169.254/latest/meta-data/", false).unwrap_err();
        assert!(err.to_string().contains("loopback"));
        assert!(validate_url("u", "https://example.com/", false).is_err());
    }

    #[test]
    fn external_targets_can_be_opted_into() {
        assert!(validate_url("u", "https://example.com/", true).is_ok());
    }

    #[test]
    fn non_http_schemes_are_refused() {
        assert!(validate_url("u", "file:///etc/passwd", true).is_err());
        assert!(validate_url("u", "gopher://127.0.0.1/", true).is_err());
    }

    #[test]
    fn credentials_in_the_url_are_refused() {
        let err = validate_url("u", "http://user:pass@127.0.0.1/", false).unwrap_err();
        assert!(err.to_string().contains("headers_from_env"));
    }

    #[test]
    fn relative_urls_are_refused() {
        assert!(validate_url("u", "/health", false).is_err());
    }

    #[test]
    fn credential_headers_must_come_from_the_environment() {
        for name in ["Authorization", "cookie", "X-API-Key"] {
            assert!(
                validate_literal_header("h", name, "value").is_err(),
                "{name}"
            );
        }
        assert!(validate_env_header("h", "Authorization", "APP_TOKEN").is_ok());
    }

    #[test]
    fn header_injection_is_refused() {
        assert!(validate_literal_header("h", "X-Trace", "a\r\nX-Evil: 1").is_err());
        assert!(validate_literal_header("h", "X-Trace\r\nEvil", "1").is_err());
    }

    #[test]
    fn ordinary_headers_are_accepted() {
        assert!(validate_literal_header("h", "Accept", "application/json").is_ok());
    }

    #[test]
    fn env_header_names_are_validated() {
        assert!(validate_env_header("h", "Authorization", "lowercase").is_err());
        assert!(validate_env_header("h", "Authorization", "").is_err());
    }
}
