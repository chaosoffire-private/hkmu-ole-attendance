//! The gateway ports: reading the timetable, and driving attendance.

use super::error::PortError;
use crate::domain::attendance::Submission;
use crate::domain::geo::Coordinates;
use crate::domain::schedule::{ScheduledClass, TodayClassResponse};

/// Reads the day's timetable from the remote service.
pub trait ScheduleGateway: Send + Sync {
    /// Retrieve the timetable the service currently reports.
    fn today_classes(
        &self,
    ) -> impl std::future::Future<Output = Result<TodayClassResponse, PortError>> + Send;
}

/// Drives attendance for a class: locating the activity, then optionally
/// recording it.
///
/// A call authenticates first, reusing a cached session when possible.
pub trait AttendanceGateway: Send + Sync {
    /// Report whether `class` has a reachable attendance activity.
    ///
    /// Read-only by contract: implementations must not submit attendance.
    /// Returning [`PortError::Session`] tells the caller the cached session was
    /// rejected, so it should be discarded and re-established.
    fn probe(
        &self,
        class: &ScheduledClass,
    ) -> impl std::future::Future<Output = Result<ActivityReport, PortError>> + Send;

    /// Attempt attendance once for `class`.
    ///
    /// Returning [`PortError::Session`] tells the caller the cached session was
    /// rejected, so it should be discarded and re-established.
    fn submit(
        &self,
        class: &ScheduledClass,
        coordinates: Option<Coordinates>,
    ) -> impl std::future::Future<Output = Result<Submission, PortError>> + Send;
}

/// What a read-only probe found on a class's activities page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivityReport {
    /// Whether an attendance-type activity exists at all.
    pub found: bool,
    /// Whether the located activity reports an open window.
    pub open: bool,
}
