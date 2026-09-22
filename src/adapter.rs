//! Adapters: the concrete implementations of the ports.
//!
//! Each adapter binds one port to something real — [`crate::infra`] for the
//! portal's HTTP, the system clock, or a Discord webhook. Nothing here is
//! referenced by [`crate::usecase`] or [`crate::domain`] — the dependency
//! points inward.

pub mod clock;
pub mod notifier;
pub mod ole_gateway;

pub use clock::SystemClock;
pub use notifier::{DiscordNotifier, LogNotifier};
pub use ole_gateway::{CacheSessionInvalidator, OleAttendanceGateway, OleScheduleGateway};
