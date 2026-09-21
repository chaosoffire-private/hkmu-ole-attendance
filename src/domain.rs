//! The pure core: values and rules, with no I/O, no clock, and no `async`.
//!
//! Nothing in this module knows that HTTP, files or Discord exist. Time is
//! always supplied by the caller, which is what makes the decision logic
//! testable without a network or a real clock.

pub mod attendance;
pub mod error;
pub mod schedule;
pub mod time;

pub use error::DomainError;
