//! The notification port.

use super::error::PortError;

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
/// and how loudly. Delivery must never be fatal — see [`crate::port::error`].
pub trait Notifier: Send + Sync {
    /// Deliver `message` at the given importance.
    fn notify(
        &self,
        level: Notice,
        message: &str,
    ) -> impl std::future::Future<Output = Result<(), PortError>> + Send;
}
