//! The gateway ports: reading the timetable, and driving attendance.

use super::error::PortError;
use crate::domain::attendance::Submission;
use crate::domain::geo::Coordinates;
use crate::domain::schedule::{ScheduledClass, Timetable};

/// Reads the day's timetable from the remote service.
pub trait ScheduleGateway: Send + Sync {
    /// Retrieve the timetable the service currently reports.
    fn today_classes(
        &self,
    ) -> impl std::future::Future<Output = Result<Timetable, PortError>> + Send;
}

/// What a read-only probe found on a class's activities page.
///
/// One value rather than a pair of flags, so the outcomes are exhaustive and
/// mutually exclusive: an activity that is not the attendance type cannot also
/// report an open window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityState {
    /// The class has an attendance activity whose window is open.
    Open,
    /// The class has an attendance activity, but its window has closed.
    Closed,
    /// The page exposes an activity, but not the attendance type.
    Absent,
}

/// Drives attendance for a class: locating the activity, then optionally
/// recording it.
///
/// A call authenticates first, reusing a cached session when possible.
pub trait AttendanceGateway: Send + Sync {
    /// Report the state of `class`'s attendance activity.
    ///
    /// Read-only by contract: implementations must not submit attendance.
    /// Returning [`PortError::Session`] tells the caller the cached session was
    /// rejected, so it should be discarded and re-established.
    fn probe(
        &self,
        class: &ScheduledClass,
    ) -> impl std::future::Future<Output = Result<ActivityState, PortError>> + Send;

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
