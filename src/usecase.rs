//! Use cases: the application's orchestration.
//!
//! These depend on [`crate::domain`] for rules and [`crate::port`] for
//! capabilities — never on a concrete adapter. That is what makes each flow
//! testable with a scripted clock, gateway and notifier.

pub mod daily_setup;
pub mod mark_attendance;
#[cfg(test)]
pub mod test_doubles;

pub use daily_setup::{SetupOutcome, daily_setup};
pub use mark_attendance::{AttendanceOutcome, PollParts, mark_attendance};
