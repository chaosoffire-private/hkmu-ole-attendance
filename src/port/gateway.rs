//! The gateway ports: reading the timetable, and driving one attendance attempt.

use super::error::PortError;
use crate::domain::attendance::Submission;
use crate::domain::schedule::{ScheduledClass, TodayClassResponse};

/// Reads the day's timetable from the remote service.
pub trait ScheduleGateway: Send + Sync {
    /// Retrieve the timetable the service currently reports.
    fn today_classes(
        &self,
    ) -> impl std::future::Future<Output = Result<TodayClassResponse, PortError>> + Send;
}

/// Drives one attendance interaction for a class.
///
/// A call authenticates (reusing a cached session when possible), locates the
/// class's activity, and submits.
pub trait AttendanceGateway: Send + Sync {
    /// Attempt attendance once for `class`.
    ///
    /// Returning [`PortError::Session`] tells the caller the cached session was
    /// rejected, so it should be discarded and re-established.
    fn submit(
        &self,
        class: &ScheduledClass,
        coordinates: Option<(f64, f64)>,
    ) -> impl std::future::Future<Output = Result<Submission, PortError>> + Send;
}
