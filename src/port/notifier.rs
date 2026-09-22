//! The notification port.

/// How important a notice is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Notice {
    /// Normal progress.
    Info,
    /// Something deserves attention but is not fatal.
    Warning,
    /// A failure.
    Error,
}

/// Delivers a user-facing message.
///
/// Implementations decide the transport; the use cases only decide what to say
/// and how loudly.
///
/// Delivery is deliberately infallible: notifications are advisory, and a
/// transport outage must never suppress the work being reported. An
/// implementation that cannot deliver logs the problem and returns, so the
/// return type carries no error to drop.
pub trait Notifier: Send + Sync {
    /// Deliver `message` at the given importance.
    fn notify(&self, level: Notice, message: &str) -> impl std::future::Future<Output = ()> + Send;
}
