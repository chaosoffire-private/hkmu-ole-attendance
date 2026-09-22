//! The failure vocabulary of the ports.
//!
//! Adapters translate their library-specific errors into these, so a use case
//! never has to know whether a request failed in `reqwest`, in DNS, or in TLS.

use thiserror::Error;

use crate::error::AppError;

/// Anything that can go wrong below the application layer.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum PortError {
    /// The session is not accepted, or was revoked mid-flight.
    #[error("session unavailable: {0}")]
    Session(SessionFailure),

    /// The remote service answered, but with an error.
    #[error("remote service error: {0}")]
    Remote(String),

    /// A response did not have the shape the adapter expected.
    #[error("unexpected response: {0}")]
    Unexpected(String),

    /// Transport-level failure: DNS, TLS, timeout, connection reset.
    #[error("transport failure: {0}")]
    Transport(String),
}

/// Why a session could not be used, so callers can tell "log in again" from
/// "give up".
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SessionFailure {
    /// The supplied cookie was refused outright.
    #[error("session rejected: {0}")]
    Rejected(String),
    /// A session that was accepted earlier is no longer valid.
    #[error("session expired or revoked: {0}")]
    Expired(String),
    /// The session could not be established for another reason.
    #[error("session unavailable: {0}")]
    Unavailable(String),
}

impl PortError {
    /// Whether the failure means the session must be re-established.
    pub const fn is_session_failure(&self) -> bool {
        matches!(self, Self::Session(_))
    }
}

/// Translate a transport-layer error into the port's vocabulary, preserving the
/// distinction between a dead session and every other failure.
impl From<AppError> for PortError {
    fn from(error: AppError) -> Self {
        match error {
            AppError::SessionRejected(reason) => Self::Session(SessionFailure::Rejected(reason)),
            AppError::SessionExpired(reason) => Self::Session(SessionFailure::Expired(reason)),
            AppError::Http(source) => Self::Transport(source.to_string()),
            AppError::Api(reason) | AppError::Parse(reason) | AppError::Config(reason) => {
                Self::Unexpected(reason)
            }
            AppError::Auth(reason) => Self::Session(SessionFailure::Unavailable(reason)),
            AppError::Json(source) => Self::Unexpected(source.to_string()),
            AppError::Url(source) => Self::Unexpected(source.to_string()),
            AppError::Time(source) => Self::Unexpected(source.to_string()),
            AppError::Domain(source) => Self::Unexpected(source.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{PortError, SessionFailure};
    use crate::error::AppError;

    #[test]
    fn a_rejected_cookie_maps_to_a_session_failure() {
        // Given the error raised while validating a supplied SESSION_COOKIE.
        let mapped = PortError::from(AppError::SessionRejected("refused".to_owned()));

        // When translated.
        // Then it stays a session failure, so the caller invalidates and retries.
        assert!(mapped.is_session_failure());
        assert_eq!(
            mapped,
            PortError::Session(SessionFailure::Rejected("refused".to_owned()))
        );
    }

    #[test]
    fn a_revoked_session_maps_to_an_expired_failure() {
        // Given the error raised when a page redirects to the login flow.
        let mapped = PortError::from(AppError::SessionExpired("revoked".to_owned()));

        // Then it remains recoverable and is classified as expired, not rejected.
        assert_eq!(
            mapped,
            PortError::Session(SessionFailure::Expired("revoked".to_owned()))
        );
    }

    #[test]
    fn transport_and_api_errors_are_not_session_failures() {
        // Given a parse failure and an API rejection.
        let parse = PortError::from(AppError::Parse("no activity".to_owned()));
        let api = PortError::from(AppError::Api("9901".to_owned()));

        // Then neither is treated as a dead session, so a healthy cached session
        // is not thrown away over an unrelated hiccup.
        assert!(!parse.is_session_failure());
        assert!(!api.is_session_failure());
        assert_eq!(parse, PortError::Unexpected("no activity".to_owned()));
    }
}
