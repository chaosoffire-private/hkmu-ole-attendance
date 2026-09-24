//! The OLE adapter: implements the gateway and session ports over HTTPS.
//!
//! This is where domain values are translated into what the HTTP client
//! expects. Nothing in the layers above needs to know these types exist.

use tracing::{debug, warn};

use crate::domain::attendance::Submission;
use crate::domain::geo::Coordinates;
use crate::domain::schedule::{ScheduledClass, Timetable};
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

/// Build the URL that records attendance.
///
/// The activity page declares its own `take_url`, already carrying
/// `createdocument` and the document id, so it is preferred: it is what a
/// browser would request, and it cannot drift from the page's layout.
///
/// When the page declared none, the request is built from the class and the
/// discovered document id. That fallback replaces the page's query rather than
/// appending to it: `readform&?createdocument` would leave `createdocument` as
/// part of a parameter value, so the server would answer a read and never see a
/// create — which is indistinguishable from a closed window.
fn submit_url(
    ole_url: &str,
    class: &ScheduledClass,
    activity: &Activity,
    coordinates: Option<Coordinates>,
) -> Result<String, PortError> {
    let point = coordinates.unwrap_or(Coordinates::ORIGIN);
    let suffix = format!("lat={}&lng={}", point.latitude(), point.longitude());

    if let Some(declared) = activity.take_url.as_deref().filter(|u| !u.is_empty()) {
        let resolved = resolve_url(ole_url, declared)?;
        return Ok(join_query(&resolved, &suffix));
    }

    let page = activities_url(ole_url, class);
    let base = page.split('?').next().unwrap_or(&page);
    Ok(format!(
        "{base}?createdocument&puid={}&{suffix}",
        activity.unid
    ))
}

/// Resolve a possibly-relative URL against the portal base.
fn resolve_url(base: &str, reference: &str) -> Result<String, PortError> {
    Ok(url::Url::parse(base)
        .and_then(|parsed| parsed.join(reference))
        .map_err(|error| PortError::Remote(format!("unusable submit URL {reference}: {error}")))?
        .to_string())
}

/// Append `extra` to a URL's query string, using `?` or `&` as required.
fn join_query(url: &str, extra: &str) -> String {
    let separator = if url.contains('?') { '&' } else { '?' };
    format!("{url}{separator}{extra}")
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
    async fn today_classes(&self) -> Result<Timetable, PortError> {
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

        if !activity.is_attendance() {
            return Err(PortError::Unexpected(format!(
                "activity type {} is not an attendance activity",
                activity.attendance_type
            )));
        }

        let submit_url = submit_url(&self.ole_url, class, &activity, coordinates)?;
        let session = self.cache.session().await?;
        let mut client = OleClient::new(session, self.api_url.clone());
        client
            .submit_attendance(&submit_url)
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
    use super::{activities_url, submit_url};
    use crate::domain::geo::Coordinates;
    use crate::domain::schedule::ScheduledClass;
    use crate::infra::oleconnect::Activity;
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

    fn activity(take_url: Option<&str>) -> Activity {
        Activity {
            unid: "ABC123".to_owned(),
            attendance_type: "2".to_owned(),
            open: true,
            take_url: take_url.map(ToOwned::to_owned),
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
    fn prefers_the_submit_url_the_page_declares() {
        // Given an activity page that declared its own take_url, as the live
        // page does.
        let declared =
            "/course2604/ELEC3050SEF.nsf/class_activities_student?createdocument&puid=ABC";

        // When the submit URL is built.
        let built = submit_url(
            "https://iole.hkmu.edu.hk",
            &class(),
            &activity(Some(declared)),
            Some(Coordinates::new(22.3364, 114.1796).expect("valid")),
        )
        .expect("builds");

        // Then it is the declared request, resolved against the portal and
        // carrying the coordinates — not a re-invented path.
        assert!(
            built.starts_with(
                "https://iole.hkmu.edu.hk/course2604/ELEC3050SEF.nsf/class_activities_student?"
            ),
            "got {built}"
        );
        assert!(built.contains("createdocument"));
        assert!(built.contains("puid=ABC"));
        assert!(built.contains("lat=22.3364"));
        assert!(built.contains("lng=114.1796"));
    }

    #[test]
    fn the_fallback_never_asks_to_read_and_create_at_once() {
        // Given an activity page that declared no take_url.
        // When the request is built from the class instead.
        let built = submit_url("https://iole.hkmu.edu.hk", &class(), &activity(None), None)
            .expect("builds");

        // Then the page's own query is replaced, not appended to. Appending
        // would produce `readform&?createdocument`, leaving the create as part
        // of a parameter value: the server answers a read and never records
        // anything, which looks exactly like a closed window.
        assert!(
            !built.contains("readform"),
            "a read request must not be combined with the create: {built}"
        );
        assert_eq!(
            built.matches('?').count(),
            1,
            "a URL carries one query string; two means a parameter value: {built}"
        );
        assert!(built.contains("?createdocument&puid=ABC123"), "got {built}");
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
