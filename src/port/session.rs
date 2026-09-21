//! The session port: where an authenticated session comes from.

use super::error::PortError;

/// The cookie header an authenticated request must carry.
///
/// Deliberately opaque: nothing above this port should inspect or log the
/// value, which keeps credentials out of the application layer entirely.
#[derive(Clone, PartialEq, Eq)]
pub struct CookieHeader(String);

impl CookieHeader {
    /// Wrap a raw `Cookie:` header value.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Borrow the value. Intended for the transport adapter only.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for CookieHeader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CookieHeader(***)")
    }
}

/// Provides an authenticated session, reusing a cached one while it is valid.
pub trait SessionProvider: Send + Sync {
    /// Obtain a usable session, logging in only when necessary.
    fn session(&self) -> impl std::future::Future<Output = Result<CookieHeader, PortError>> + Send;

    /// Discard any cached session, forcing the next call to log in again.
    fn invalidate(&self) -> impl std::future::Future<Output = ()> + Send;
}
