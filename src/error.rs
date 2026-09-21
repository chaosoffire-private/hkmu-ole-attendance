//! Typed error surface for the OLE attendance client.

use thiserror::Error;

/// Every failure mode this crate can surface.
#[derive(Debug, Error)]
pub enum AppError {
    /// A required environment variable is missing or malformed.
    #[error("configuration error: {0}")]
    Config(String),

    /// The SSO login chain did not reach an authenticated dashboard.
    #[error("authentication failed: {0}")]
    Auth(String),

    /// The OLE API answered, but with an error payload.
    #[error("OLE API error: {0}")]
    Api(String),

    /// A page did not contain the markers the flow depends on.
    #[error("unexpected page layout: {0}")]
    Parse(String),

    /// The configured session cookie was rejected by OLE.
    #[error("session cookie rejected: {0}")]
    SessionRejected(String),

    /// OLE redirected a page request to its login flow because the session is
    /// no longer accepted, even if the expiry we cached still looked valid.
    ///
    /// Distinct from [`AppError::SessionRejected`], which is raised only while
    /// validating a freshly supplied `SESSION_COOKIE`.
    #[error("session expired or was revoked: {0}")]
    SessionExpired(String),

    /// Transport-level failure.
    #[error(transparent)]
    Http(#[from] reqwest::Error),

    /// JSON decoding failure.
    #[error(transparent)]
    Json(#[from] serde_json::Error),

    /// URL construction failure.
    #[error(transparent)]
    Url(#[from] url::ParseError),

    /// Date/time parsing or arithmetic failure.
    #[error(transparent)]
    Time(#[from] jiff::Error),

    /// A domain rule rejected a value.
    #[error(transparent)]
    Domain(#[from] crate::domain::DomainError),
}

/// Convenience alias for fallible operations in this crate.
pub type Result<T> = std::result::Result<T, AppError>;
