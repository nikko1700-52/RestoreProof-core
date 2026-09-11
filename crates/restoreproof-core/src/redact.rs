//! Redaction helpers applied to everything that leaves the process.
//!
//! Two complementary mechanisms are used:
//!
//! 1. **Known-value redaction** — every secret resolved during a run (restic
//!    repository password, database DSN, ...) is registered with the
//!    [`Redactor`], which then removes it from logs, check output and reports.
//! 2. **Structural scrubbing** — [`scrub_url_credentials`] removes the
//!    `user:password@` part of any URL, which catches credentials that were
//!    never registered (for instance one printed by a failing external tool).

use std::collections::BTreeSet;

/// Marker written in place of a redacted value.
pub const REDACTED: &str = "[REDACTED]";

/// Secrets shorter than this are not registered for literal replacement: a
/// two-character password would otherwise mangle every report it appears in.
/// Such secrets are still never printed, because they never leave [`crate::Secret`].
const MIN_REDACTABLE_LEN: usize = 4;

/// Removes known secrets and URL credentials from arbitrary text.
///
/// Cloning a `Redactor` is cheap enough for per-check use and it is `Send +
/// Sync`, so the same instance can be shared by the runner and the reporter.
#[derive(Debug, Clone, Default)]
pub struct Redactor {
    /// Ordered longest-first so that overlapping secrets redact greedily.
    secrets: Vec<String>,
    seen: BTreeSet<String>,
}

impl Redactor {
    /// Create an empty redactor.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a value to be removed from every future output.
    ///
    /// Values shorter than four bytes are ignored on purpose (see
    /// [`MIN_REDACTABLE_LEN`]); whitespace-only values are ignored as well.
    pub fn add_secret(&mut self, value: &str) {
        let trimmed = value.trim();
        if trimmed.len() < MIN_REDACTABLE_LEN || !self.seen.insert(trimmed.to_owned()) {
            return;
        }
        self.secrets.push(trimmed.to_owned());
        self.secrets.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
    }

    /// Number of registered secrets. Used by tests and by `--verbose` output.
    #[must_use]
    pub fn registered(&self) -> usize {
        self.secrets.len()
    }

    /// Redact a single string.
    #[must_use]
    pub fn redact(&self, input: &str) -> String {
        let mut out = input.to_owned();
        for secret in &self.secrets {
            if out.contains(secret.as_str()) {
                out = out.replace(secret.as_str(), REDACTED);
            }
        }
        scrub_url_credentials(&out)
    }

    /// Redact and truncate, the combination used for captured process output.
    #[must_use]
    pub fn redact_truncated(&self, input: &str, max_bytes: usize) -> String {
        truncate(&self.redact(input), max_bytes)
    }
}

/// Replace the password component of every `scheme://user:password@host` URL.
///
/// The parser is intentionally permissive: it operates on raw text (log lines,
/// error messages) rather than on well-formed URLs.
#[must_use]
pub fn scrub_url_credentials(input: &str) -> String {
    const AUTHORITY_TERMINATORS: [char; 8] = ['/', '?', '#', ' ', '\t', '\n', '"', '\''];

    let mut out = String::with_capacity(input.len());
    let mut rest = input;

    while let Some(pos) = rest.find("://") {
        let Some(head) = rest.get(..pos + 3) else { break };
        let Some(tail) = rest.get(pos + 3..) else { break };
        out.push_str(head);

        let auth_end = tail.find(AUTHORITY_TERMINATORS).unwrap_or(tail.len());
        let authority = tail.get(..auth_end).unwrap_or_default();
        let remainder = tail.get(auth_end..).unwrap_or_default();

        match authority.rfind('@') {
            Some(at) => {
                let userinfo = authority.get(..at).unwrap_or_default();
                let host = authority.get(at..).unwrap_or_default();
                if let Some(colon) = userinfo.find(':') {
                    out.push_str(userinfo.get(..colon).unwrap_or_default());
                    out.push(':');
                    out.push_str(REDACTED);
                } else {
                    out.push_str(userinfo);
                }
                out.push_str(host);
            }
            None => out.push_str(authority),
        }

        rest = remainder;
    }

    out.push_str(rest);
    out
}

/// Truncate `input` to at most `max_bytes`, never splitting a UTF-8 character.
///
/// Used to bound the size of captured command output so that a runaway process
/// cannot produce a multi-gigabyte report.
#[must_use]
pub fn truncate(input: &str, max_bytes: usize) -> String {
    if input.len() <= max_bytes {
        return input.to_owned();
    }
    let mut end = max_bytes;
    while end > 0 && !input.is_char_boundary(end) {
        end -= 1;
    }
    let head = input.get(..end).unwrap_or_default();
    format!(
        "{head}\n[... truncated, {} bytes total, {} bytes kept]",
        input.len(),
        end
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registered_secrets_are_removed() {
        let mut redactor = Redactor::new();
        redactor.add_secret("s3cret-password");
        let text = "connecting with s3cret-password to the database";
        assert_eq!(
            redactor.redact(text),
            "connecting with [REDACTED] to the database"
        );
    }

    #[test]
    fn short_secrets_are_not_registered() {
        let mut redactor = Redactor::new();
        redactor.add_secret("ab");
        redactor.add_secret("   ");
        assert_eq!(redactor.registered(), 0);
        assert_eq!(redactor.redact("ab cd"), "ab cd");
    }

    #[test]
    fn longest_secret_wins_on_overlap() {
        let mut redactor = Redactor::new();
        redactor.add_secret("token");
        redactor.add_secret("token-extended");
        assert_eq!(redactor.redact("token-extended"), REDACTED);
    }

    #[test]
    fn url_passwords_are_scrubbed_even_when_unknown() {
        let input = "postgres://app:Sup3rS3cret@db.internal:5432/app?sslmode=require";
        let scrubbed = scrub_url_credentials(input);
        assert_eq!(
            scrubbed,
            "postgres://app:[REDACTED]@db.internal:5432/app?sslmode=require"
        );
        assert!(!scrubbed.contains("Sup3rS3cret"));
    }

    #[test]
    fn url_without_credentials_is_untouched() {
        let input = "http://127.0.0.1:8080/health";
        assert_eq!(scrub_url_credentials(input), input);
    }

    #[test]
    fn several_urls_on_one_line_are_all_scrubbed() {
        let input = "a=redis://u:p4ssword@h:6379 b=amqp://u2:p4ssword2@h:5672/";
        let scrubbed = scrub_url_credentials(input);
        assert!(!scrubbed.contains("p4ssword"));
        assert!(!scrubbed.contains("p4ssword2"));
        assert_eq!(scrubbed.matches(REDACTED).count(), 2);
    }

    #[test]
    fn userinfo_without_password_is_preserved() {
        let input = "ssh://operator@backup-host/repo";
        assert_eq!(scrub_url_credentials(input), input);
    }

    #[test]
    fn truncate_respects_char_boundaries() {
        let input = "é".repeat(10); // 20 bytes
        let out = truncate(&input, 5);
        assert!(out.starts_with("éé"));
        assert!(out.contains("truncated"));
        assert!(out.is_char_boundary(0));
    }

    #[test]
    fn truncate_is_a_noop_for_short_input() {
        assert_eq!(truncate("short", 64), "short");
    }
}
