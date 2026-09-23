//! The OLE adapter: implements the gateway and session ports over HTTPS.
//!
//! This is where domain values are translated into what the HTTP client
//! expects. Nothing in the layers above needs to know these types exist.

use tracing::{debug, warn};

use crate::domain::attendance::Submission;
use crate::domain::geo::Coordinates;
use crate::domain::schedule::{ScheduledClass, TodayClassResponse};
use crate::infra::oleconnect::{Activity, OleClient};
use crate::infra::session_cache::SessionCache;
use crate::port::error::PortError;
use crate::port::gateway::ActivityState;
use crate::port::{AttendanceGateway, ScheduleGateway, SessionInvalidator};

/// The class-activities page for a course.
fn activities_url(ole_url: &str, class: &ScheduledClass) -> String {
    let base = ole_url.trim_end_matches('/');
    format!(
        "{base}/course{}/{}.nsf//class_activities_student?readform&",
        class.termcode, class.course_code
    )
}

/// Reads the timetable over HTTPS.
#[derive(Debug)]
pub struct OleScheduleGateway {
    cache: SessionCache,
    api_url: String,
}

impl OleScheduleGateway {
    /// Build a gateway that authenticates through `cache`.
    pub const fn new(cache: SessionCache, api_url: String) -> Self {
        Self { cache, api_url }
    }
}

impl ScheduleGateway for OleScheduleGateway {
    async fn today_classes(&self) -> Result<TodayClassResponse, PortError> {
        let session = self.cache.session().await?;
        let mut client = OleClient::new(session, self.api_url.clone());
        let payload = client.fetch_today_classes().await?;

        // The port speaks the domain type; the wire type is an adapter concern.
        Ok(payload)
    }
}

/// Submits attendance over HTTPS, reusing a cached session between polls.
#[derive(Debug)]
pub struct OleAttendanceGateway {
    cache: SessionCache,
    api_url: String,
    ole_url: String,
}

impl OleAttendanceGateway {
    /// Build a gateway that authenticates through `cache`.
    pub const fn new(cache: SessionCache, api_url: String, ole_url: String) -> Self {
        Self {
            cache,
            api_url,
            ole_url,
        }
    }
}

impl AttendanceGateway for OleAttendanceGateway {
    async fn probe(&self, class: &ScheduledClass) -> Result<ActivityState, PortError> {
        let activity = self.locate(class).await?;
        Ok(if !activity.is_attendance() {
            ActivityState::Absent
        } else if activity.open {
            ActivityState::Open
        } else {
            ActivityState::Closed
        })
    }

    async fn submit(
        &self,
        class: &ScheduledClass,
        coordinates: Option<Coordinates>,
    ) -> Result<Submission, PortError> {
        let activity = self.locate(class).await?;
        let activities = activities_url(&self.ole_url, class);

        if !activity.is_attendance() {
            return Err(PortError::Unexpected(format!(
                "activity type {} is not an attendance activity",
                activity.attendance_type
            )));
        }

        let session = self.cache.session().await?;
        let mut client = OleClient::new(session, self.api_url.clone());
        client
            .submit_attendance(&activities, &activity.unid, coordinates)
            .await
            .map_err(PortError::from)
    }
}

impl OleAttendanceGateway {
    async fn locate(&self, class: &ScheduledClass) -> Result<Activity, PortError> {
        let session = self.cache.session().await?;
        let mut client = OleClient::new(session, self.api_url.clone());
        let activities = activities_url(&self.ole_url, class);

        match client.discover_activity(class, &activities).await {
            Ok(activity) => {
                debug!(unid = %activity.unid, kind = %activity.attendance_type, "activity located");
                Ok(activity)
            }
            Err(error) => {
                let mapped = PortError::from(error);
                // A revoked session is recoverable: the port reports it as a
                // session failure so the use case discards the cache and logs in
                // again rather than replaying a dead cookie.
                if mapped.is_session_failure() {
                    warn!(%mapped, "session was revoked server-side");
                }
                Err(mapped)
            }
        }
    }
}

/// Exposes the cache's invalidation to the application without leaking the
/// cache itself, so the use cases never name a transport type.
#[derive(Debug)]
pub struct CacheSessionInvalidator {
    cache: SessionCache,
}

impl CacheSessionInvalidator {
    /// Wrap an existing cache.
    pub const fn new(cache: SessionCache) -> Self {
        Self { cache }
    }
}

impl SessionInvalidator for CacheSessionInvalidator {
    async fn invalidate(&self) {
        self.cache.invalidate().await;
    }
}

#[cfg(test)]
mod tests {
    use super::activities_url;
    use crate::domain::schedule::ScheduledClass;
    use crate::port::gateway::ActivityState;

    fn class() -> ScheduledClass {
        ScheduledClass {
            termcode: "2604".to_owned(),
            course_code: "ELEC3050SEF".to_owned(),
            name: "Lecture ( Full Time )".to_owned(),
            starts_at: jiff::civil::date(2026, 9, 21).at(11, 0, 0, 0),
            ends_at: Some(jiff::civil::date(2026, 9, 21).at(12, 50, 0, 0)),
            group: "L01".to_owned(),
            venue: "HKMU C0G01".to_owned(),
        }
    }

    #[test]
    fn builds_the_class_activities_url_from_the_domain_class() {
        // Given a scheduled class and the portal base.
        // When the activities URL is built.
        // Then it matches the path the site serves, without a duplicate builder.
        assert_eq!(
            activities_url("https://iole.hkmu.edu.hk/", &class()),
            "https://iole.hkmu.edu.hk/course2604/ELEC3050SEF.nsf//class_activities_student?readform&"
        );
    }

    #[test]
    fn the_activity_state_distinguishes_absent_closed_and_open() {
        // Given the three outcomes a probe can report.
        let open = ActivityState::Open;
        let closed = ActivityState::Closed;
        let absent = ActivityState::Absent;

        // When compared.
        // Then each is distinct, so the operator can tell "no activity" from
        // "activity exists but its window has closed".
        assert_ne!(open, closed);
        assert_ne!(closed, absent);
        assert_ne!(open, absent);
    }
}
