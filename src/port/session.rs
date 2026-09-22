//! The session port: telling the application when a cached session is dead.

/// Tracks the authenticated session the transports reuse between requests.
///
/// Only invalidation crosses this port. Obtaining the session itself is a
/// transport concern — an adapter holds the real credentials and the HTTP
/// client — so the application never handles a cookie value.
pub trait SessionInvalidator: Send + Sync {
    /// Discard any cached session, forcing the next call to log in again.
    fn invalidate(&self) -> impl std::future::Future<Output = ()> + Send;
}
