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
            AppError::Http(source) => Self::Transport(describe(&source)),
            AppError::Api(reason) => Self::Remote(reason),
            AppError::HttpStatus { status, url } => {
                Self::Remote(format!("HTTP {status} from {url}"))
            }
            AppError::Parse(reason) | AppError::Config(reason) => Self::Unexpected(reason),
            AppError::Auth(reason) => Self::Session(SessionFailure::Unavailable(reason)),
            AppError::Json(source) => Self::Unexpected(source.to_string()),
            AppError::Url(source) => Self::Unexpected(source.to_string()),
            AppError::Time(source) => Self::Unexpected(source.to_string()),
            AppError::Domain(source) => Self::Unexpected(source.to_string()),
        }
    }
}

/// Render an error together with its source chain.
///
/// `reqwest::Error`'s own `Display` names only the request kind and URL, so a
/// timeout, a TLS rejection and a DNS failure would otherwise collapse into one
/// indistinguishable message. In a `FROM scratch` container the log is the only
/// diagnostic, so the cause must be carried across the port boundary.
fn describe(error: &dyn std::error::Error) -> String {
    let mut rendered = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        rendered.push_str(": ");
        rendered.push_str(&cause.to_string());
        source = cause.source();
    }
    rendered
}

#[cfg(test)]
mod tests {
    use super::{PortError, SessionFailure, describe};
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

    #[test]
    fn an_api_rejection_stays_a_remote_error() {
        // Given the API signalling that the session is not permitted.
        let mapped = PortError::from(AppError::Api("9901: only for web users".to_owned()));

        // Then it is a remote error, not an "unexpected response": the server
        // answered, and the operator needs to know that distinction.
        assert_eq!(
            mapped,
            PortError::Remote("9901: only for web users".to_owned())
        );
    }

    #[test]
    fn a_non_success_status_is_reported_as_a_remote_error() {
        // Given the server answering with an HTTP error status.
        let mapped = PortError::from(AppError::HttpStatus {
            status: 503,
            url: "https://oleconnect.hkmu.edu.hk/x".to_owned(),
        });

        // Then the status survives into the port vocabulary, so a 5xx is not
        // mistaken for a page whose layout changed.
        assert!(!mapped.is_session_failure());
        assert!(
            matches!(&mapped, PortError::Remote(reason) if reason.contains("503")),
            "got {mapped:?}"
        );
    }

    #[test]
    fn a_transport_failure_keeps_its_cause_chain() {
        // Given a transport error wrapping an inner cause.
        #[derive(Debug)]
        struct Cause;
        impl std::fmt::Display for Cause {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("certificate expired")
            }
        }
        impl std::error::Error for Cause {}

        #[derive(Debug)]
        struct Outer(Cause);
        impl std::fmt::Display for Outer {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("error sending request")
            }
        }
        impl std::error::Error for Outer {
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                Some(&self.0)
            }
        }

        // When rendered with the chain.
        let rendered = describe(&Outer(Cause));

        // Then both the outer message and the cause appear, so a timeout can be
        // told from a TLS rejection in an environment with no backtrace.
        assert!(rendered.contains("error sending request"));
        assert!(rendered.contains("certificate expired"));
    }
}
