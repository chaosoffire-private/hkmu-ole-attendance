//! The OLE adapter: implements the gateway and session ports over HTTPS.
//!
//! This is where domain values are translated into what the HTTP client
//! expects. Nothing in the layers above needs to know these types exist.

use tracing::{debug, warn};

use crate::domain::attendance::Submission;
use crate::domain::schedule::{ScheduledClass, TodayClassResponse};
use crate::error::AppError;
use crate::models::ClassInfo;
use crate::oleconnect::OleClient;
use crate::port::error::PortError;
use crate::port::session::CookieHeader;
use crate::port::{AttendanceGateway, ScheduleGateway, SessionProvider};
use crate::session_cache::SessionCache;

/// Build the class-activities URL for a course.
fn activities_url(ole_url: &str, class: &ScheduledClass) -> String {
    let base = ole_url.trim_end_matches('/');
    format!(
        "{base}/course{}/{}.nsf//class_activities_student?readform&",
        class.termcode, class.course_code
    )
}

/// Translate a domain class into the shape the HTTP client consumes.
fn to_client_class(ole_url: &str, class: &ScheduledClass) -> ClassInfo {
    ClassInfo {
        termcode: class.termcode.clone(),
        course_code: class.course_code.clone(),
        class_name: class.name.clone(),
        starts_at: class.starts_at,
        ends_at: class.ends_at,
        group: class.group.clone(),
        venue: class.venue.clone(),
        activities_url: activities_url(ole_url, class),
    }
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
        let session = self
            .cache
            .session()
            .await
            .map_err(|error| PortError::Session(error.to_string()))?;
        let mut client = OleClient::new(session, self.api_url.clone());
        let payload = client
            .fetch_today_classes()
            .await
            .map_err(|error| PortError::Remote(error.to_string()))?;

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
    async fn submit(
        &self,
        class: &ScheduledClass,
        coordinates: Option<(f64, f64)>,
    ) -> Result<Submission, PortError> {
        let session = self
            .cache
            .session()
            .await
            .map_err(|error| PortError::Session(error.to_string()))?;
        let mut client = OleClient::new(session, self.api_url.clone());
        let client_class = to_client_class(&self.ole_url, class);

        let activity =
            client
                .discover_activity(&client_class)
                .await
                .map_err(|error| match error {
                    // A revoked session is recoverable: surface it as such so the
                    // use case discards the cache and logs in again.
                    AppError::SessionExpired(reason) => {
                        warn!(%reason, "session was revoked server-side");
                        PortError::Session(reason)
                    }
                    other => PortError::Unexpected(other.to_string()),
                })?;

        debug!(unid = %activity.unid, kind = %activity.attendance_type, "activity located");

        if !activity.is_attendance() {
            return Err(PortError::Unexpected(format!(
                "activity type {} is not an attendance activity",
                activity.attendance_type
            )));
        }

        client
            .submit_attendance(&client_class.activities_url, &activity.unid, coordinates)
            .await
            .map_err(|error| PortError::Remote(error.to_string()))
    }
}

/// Adapts the caching authenticator to the session port.
#[derive(Debug)]
pub struct CachingSessionProvider {
    cache: SessionCache,
}

impl CachingSessionProvider {
    /// Wrap an existing cache.
    pub const fn new(cache: SessionCache) -> Self {
        Self { cache }
    }
}

impl SessionProvider for CachingSessionProvider {
    /// The cookie itself stays inside the cache; the application layer only
    /// learns whether a session is available.
    async fn session(&self) -> Result<CookieHeader, PortError> {
        self.cache
            .session()
            .await
            .map(|_| CookieHeader::new("cached"))
            .map_err(|error| PortError::Session(error.to_string()))
    }

    async fn invalidate(&self) {
        self.cache.invalidate().await;
    }
}
