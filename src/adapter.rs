//! Adapters: the concrete implementations of the ports.
//!
//! This is the only layer that knows about `reqwest`, the system clock, or a
//! Discord webhook. Nothing here is referenced by [`crate::usecase`] or
//! [`crate::domain`] — the dependency points inward.

pub mod clock;
pub mod notifier;
pub mod ole_gateway;

pub use clock::SystemClock;
pub use notifier::{CompositeNotifier, DiscordNotifier, LogNotifier};
pub use ole_gateway::{CachingSessionProvider, OleAttendanceGateway, OleScheduleGateway};
