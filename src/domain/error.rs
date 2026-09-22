//! Errors raised by domain rules, with no dependency on I/O.

use thiserror::Error;

/// A value the domain refuses to construct.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum DomainError {
    /// A configured time of day could not be parsed.
    #[error("invalid time of day {raw:?}: {reason}")]
    TimeOfDay {
        /// The rejected input.
        raw: String,
        /// Why it was rejected.
        reason: &'static str,
    },

    /// A timestamp from an external source had the wrong shape.
    #[error("invalid timestamp {raw:?}")]
    Timestamp {
        /// The rejected input.
        raw: String,
    },

    /// A coordinate pair the domain refused to construct.
    #[error("invalid coordinates: {reason}")]
    InvalidCoordinates {
        /// Why the pair was rejected.
        reason: &'static str,
    },
}
