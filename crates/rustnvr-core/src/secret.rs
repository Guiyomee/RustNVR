//! Secret values that must never be written to logs.

use serde::Deserialize;
use std::fmt;

/// A secret string such as a password.
///
/// `Debug` and `Display` never reveal the value; use [`Secret::expose`] at the
/// single place where the value is handed to a protocol library. `Serialize`
/// is intentionally not implemented so that a secret cannot leak through a
/// serialized configuration or API response by accident.
#[derive(Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(transparent)]
pub struct Secret(String);

impl Secret {
    /// Wraps a value.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Returns the secret value. Call sites should be few and easy to audit.
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// Returns true if the secret is the empty string.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(***)")
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("***")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn never_formats_value() {
        let s = Secret::new("hunter2");
        assert_eq!(format!("{s}"), "***");
        assert!(!format!("{s:?}").contains("hunter2"));
        assert_eq!(s.expose(), "hunter2");
    }
}
