//! Time as seen by the application.
//!
//! Injectable so the polling policy can be tested without waiting for real
//! time to pass.

use jiff::{Timestamp, Zoned};

/// Supplies the current instant and the zone it should be read in.
pub trait Clock: Send + Sync + std::fmt::Debug {
    /// The current instant, in `zone`.
    fn now(&self, zone: &jiff::tz::TimeZone) -> Zoned;

    /// The current instant, as a bare timestamp.
    fn timestamp(&self) -> Timestamp {
        Timestamp::now()
    }
}
