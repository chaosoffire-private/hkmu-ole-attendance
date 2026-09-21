//! Secret wrapper that never leaks through `Debug` or `Display`.

use std::fmt;

/// A string whose value is redacted in every diagnostic representation.
///
/// Wrap credentials and session cookies so an accidental `tracing::debug!`
/// or `{:?}` on a config struct cannot spill them into logs.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    /// Wrap a raw secret value.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Borrow the underlying value. Call sites should stay few and auditable.
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// Whether the secret has no meaningful content.
    pub fn is_empty(&self) -> bool {
        self.0.trim().is_empty()
    }
}

impl From<String> for Secret {
    fn from(value: String) -> Self {
        Self(value)
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
    use super::Secret;

    #[test]
    fn debug_and_display_never_reveal_the_value() {
        // Given a secret holding a recognisable value.
        let secret = Secret::new("hunter2");

        // When it is rendered for diagnostics.
        let debug = format!("{secret:?}");
        let display = format!("{secret}");

        // Then the value appears in neither rendering.
        assert_eq!(debug, "Secret(***)");
        assert_eq!(display, "***");
        assert!(!debug.contains("hunter2"));
        assert!(!display.contains("hunter2"));
    }

    #[test]
    fn expose_returns_the_original_value() {
        // Given a wrapped secret.
        let secret = Secret::new("abc");

        // When explicitly exposed.
        // Then the caller gets the exact original bytes.
        assert_eq!(secret.expose(), "abc");
    }

    #[test]
    fn whitespace_only_secret_counts_as_empty() {
        // Given a secret that is only whitespace.
        let secret = Secret::new("   ");

        // When emptiness is queried.
        // Then it reports empty, so callers can skip it.
        assert!(secret.is_empty());
    }
}
