//! The failure vocabulary of the ports.
//!
//! Adapters translate their library-specific errors into these, so a use case
//! never has to know whether a request failed in `reqwest`, in DNS, or in TLS.

use thiserror::Error;

/// Anything that can go wrong below the application layer.
#[derive(Debug, Error)]
pub enum PortError {
    /// The session is not accepted, or was revoked mid-flight.
    #[error("session unavailable: {0}")]
    Session(String),

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

impl PortError {
    /// Whether the failure means the session must be re-established.
    pub const fn is_session_failure(&self) -> bool {
        matches!(self, Self::Session(_))
    }
}
