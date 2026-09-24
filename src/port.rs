//! Ports: the interfaces the use cases depend on.
//!
//! Every abstraction here is owned by the application, not by a library. The
//! adapters in [`crate::adapter`] implement them, and the use cases in
//! [`crate::usecase`] depend only on these traits — so the transport, the clock
//! and the notifier can be replaced without touching a single business rule.
//!
//! The traits are consumed through generics in the use cases, so the dependency
//! is inverted with no dispatch cost on the polling paths. Where a set of
//! implementations must be held together — the notifier broadcast — the port is
//! dyn-compatible so any implementation can be composed, including another
//! broadcast. That case pays one box per delivery, which is measured in
//! microseconds against a handful of notifications a day.

pub mod clock;
pub mod error;
pub mod gateway;
pub mod notifier;
pub mod session;

pub use clock::Clock;
pub use error::PortError;

/// Convenience alias for fallible port operations.
pub type Result<T> = std::result::Result<T, PortError>;
pub use gateway::{ActivityState, AttendanceGateway, ScheduleGateway};
pub use notifier::{Notice, Notifier};
pub use session::SessionInvalidator;
