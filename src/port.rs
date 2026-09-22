//! Ports: the interfaces the use cases depend on.
//!
//! Every abstraction here is owned by the application, not by a library. The
//! adapters in [`crate::adapter`] implement them, and the use cases in
//! [`crate::usecase`] depend only on these traits — so the transport, the clock
//! and the notifier can be replaced without touching a single business rule.
//!
//! The traits use `async fn` and are consumed through generics rather than
//! `dyn`, so the dependency is inverted at compile time with no dispatch cost
//! and no extra dependency.

pub mod clock;
pub mod error;
pub mod gateway;
pub mod notifier;
pub mod session;

pub use clock::Clock;
pub use error::PortError;

/// Convenience alias for fallible port operations.
pub type Result<T> = std::result::Result<T, PortError>;
pub use gateway::{AttendanceGateway, ScheduleGateway};
pub use notifier::{Notice, Notifier};
pub use session::SessionInvalidator;
