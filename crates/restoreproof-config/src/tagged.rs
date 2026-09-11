//! Helpers for documents that use a flat `type:` discriminator.
//!
//! Serde's internally tagged enums cannot be combined with
//! `deny_unknown_fields`, and strictness matters more here than derive
//! convenience: a misspelled field in a check definition must be an error.
//!
//! These helpers deserialize into a YAML mapping, remove the discriminator and
//! then feed the remainder to a strict struct, so unknown fields are still
//! rejected and error messages carry the context of the offending item.

use serde::de::{DeserializeOwned, Error as DeError};
use serde_yaml_ng::{Mapping, Value};

/// Require the value to be a mapping.
pub(crate) fn as_mapping<E: DeError>(context: &str, value: Value) -> Result<Mapping, E> {
    match value {
        Value::Mapping(mapping) => Ok(mapping),
        other => Err(E::custom(format!(
            "{context}: expected a mapping, found {}",
            describe(&other)
        ))),
    }
}

/// Remove and return a string-valued key, failing with a helpful message.
pub(crate) fn take_tag<E: DeError>(
    context: &str,
    mapping: &mut Mapping,
    key: &str,
) -> Result<String, E> {
    let Some(value) = mapping.remove(Value::String(key.to_owned())) else {
        return Err(E::custom(format!(
            "{context}: missing required field `{key}`"
        )));
    };
    match value {
        Value::String(tag) => Ok(tag),
        other => Err(E::custom(format!(
            "{context}: field `{key}` must be a string, found {}",
            describe(&other)
        ))),
    }
}

/// Remove and return an optional string-valued key.
pub(crate) fn take_optional_string<E: DeError>(
    context: &str,
    mapping: &mut Mapping,
    key: &str,
) -> Result<Option<String>, E> {
    match mapping.remove(Value::String(key.to_owned())) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value)),
        Some(other) => Err(E::custom(format!(
            "{context}: field `{key}` must be a string, found {}",
            describe(&other)
        ))),
    }
}

/// Remove and return an optional value of any deserializable type.
pub(crate) fn take_optional<T: DeserializeOwned, E: DeError>(
    context: &str,
    mapping: &mut Mapping,
    key: &str,
) -> Result<Option<T>, E> {
    match mapping.remove(Value::String(key.to_owned())) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => serde_yaml_ng::from_value(value)
            .map(Some)
            .map_err(|err| E::custom(format!("{context}: field `{key}`: {err}"))),
    }
}

/// Deserialize the remainder of a mapping into a strict struct.
pub(crate) fn from_mapping<T: DeserializeOwned, E: DeError>(
    context: &str,
    mapping: Mapping,
) -> Result<T, E> {
    T::deserialize(Value::Mapping(mapping)).map_err(|err| E::custom(format!("{context}: {err}")))
}

fn describe(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Sequence(_) => "a list",
        Value::Mapping(_) => "a mapping",
        Value::Tagged(_) => "a tagged value",
    }
}
