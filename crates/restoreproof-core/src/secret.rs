//! A credential container that cannot be printed, logged or serialized.

use std::fmt;

use zeroize::{Zeroize, ZeroizeOnDrop};

/// A secret value held in memory for the duration of a run.
///
/// The type intentionally does **not** implement [`fmt::Display`],
/// [`serde::Serialize`] round-tripping of the plaintext, or
/// [`serde::Deserialize`]. Secrets enter the process from an environment
/// variable or from a file referenced by the configuration, never from the YAML
/// document itself, so that configuration files stay safe to commit.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct Secret {
    value: String,
}

impl Secret {
    /// Wrap a plaintext value.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self {
            value: value.into(),
        }
    }

    /// Borrow the plaintext.
    ///
    /// Every call site of this method is a place where a secret may escape, so
    /// the explicit name makes those sites easy to audit (`grep expose_secret`).
    #[must_use]
    pub fn expose_secret(&self) -> &str {
        &self.value
    }

    /// Length of the plaintext in bytes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.value.len()
    }

    /// Whether the secret is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.value.is_empty()
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret([REDACTED])")
    }
}

/// Serializing a secret always emits the redaction marker, never the value.
/// This guarantees that a report cannot leak a credential even if a future
/// refactoring embeds a [`Secret`] in a serializable structure by mistake.
impl serde::Serialize for Secret {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(crate::redact::REDACTED)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_never_prints_the_value() {
        let secret = Secret::new("hunter2-super-secret");
        assert_eq!(format!("{secret:?}"), "Secret([REDACTED])");
        assert!(!format!("{secret:?}").contains("hunter2"));
    }

    #[test]
    fn serialization_emits_the_redaction_marker() {
        let secret = Secret::new("hunter2-super-secret");
        let json = serde_json::to_string(&secret).unwrap();
        assert_eq!(json, "\"[REDACTED]\"");
    }

    #[test]
    fn plaintext_is_reachable_only_through_expose_secret() {
        let secret = Secret::new("abc");
        assert_eq!(secret.expose_secret(), "abc");
        assert_eq!(secret.len(), 3);
        assert!(!secret.is_empty());
    }
}
