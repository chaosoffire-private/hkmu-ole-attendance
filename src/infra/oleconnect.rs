//! The OLE class-activities flow: listing tasks, submitting, and confirming.

use tracing::{debug, info, warn};

use super::html;
use super::session::HttpSession;
use crate::domain::schedule::{ScheduledClass, TodayClassResponse};
use crate::error::{AppError, Result};

/// Attendance activity type used by the class-activities system.
const ATTENDANCE_TYPE: &str = "2";

/// Re-exported from the domain: the submission outcome is domain knowledge,
/// and duplicating it here risked the two definitions drifting apart.
pub use crate::domain::attendance::Submission as AttendanceOutcome;

/// An attendance activity discovered on a course page.
#[derive(Debug, Clone)]
pub struct Activity {
    /// Domino document identifier of the activity.
    pub unid: String,
    /// Raw attendance type reported by the page.
    pub attendance_type: String,
    /// Whether the page reports a running submission window.
    pub open: bool,
}

impl Activity {
    /// Whether this activity is the geolocation-backed attendance type.
    pub fn is_attendance(&self) -> bool {
        self.attendance_type == ATTENDANCE_TYPE
    }
}

/// Client for the OLE portal pages and the `oledb` API.
#[derive(Debug)]
pub struct OleClient {
    session: HttpSession,
    api_url: String,
}

impl OleClient {
    /// Wrap an authenticated session.
    #[allow(
        clippy::missing_const_for_fn,
        reason = "String cannot be moved in a const fn"
    )]
    pub fn new(session: HttpSession, api_url: String) -> Self {
        Self { session, api_url }
    }

    /// Fetch the day's timetable from the `oledb` API.
    ///
    /// # Errors
    /// Returns [`AppError::Api`] when the API rejects the session.
    pub async fn fetch_today_classes(&mut self) -> Result<TodayClassResponse> {
        let payload: TodayClassResponse = self.session.get_json(&self.api_url).await?;
        if let Some(summary) = payload.error_summary() {
            warn!(error = %summary, "OLE API returned an error payload");
        }
        Ok(payload)
    }

    /// Discover the open attendance activity on a course's activities page.
    ///
    /// # Errors
    /// Returns [`AppError::Parse`] when the page exposes neither an inline
    /// activity nor a task list.
    pub async fn discover_activity(
        &mut self,
        class: &ScheduledClass,
        activities_url: &str,
    ) -> Result<Activity> {
        let list = self.session.get(activities_url).await?;
        if html::is_login_redirect(&list) {
            return Err(AppError::SessionExpired(format!(
                "{activities_url} redirected to the login flow; the session was revoked server-side"
            )));
        }
        if let Some(activity) = activity_from_page(&list) {
            debug!(unid = %activity.unid, kind = %activity.attendance_type, "activity present inline");
            return Ok(activity);
        }

        let expected = expected_class_id(class);
        let tasks = task_links(&list);
        debug!(candidates = tasks.len(), %expected, "scanning the task list");

        let mut ordered: Vec<TaskLink> = tasks
            .iter()
            .filter(|task| task.class_id.starts_with(&expected))
            .cloned()
            .collect();
        ordered.extend(
            tasks
                .into_iter()
                .filter(|task| !task.class_id.starts_with(&expected)),
        );

        let mut fallback: Option<Activity> = None;
        for task in ordered {
            let document = self.open_activity(activities_url, &task.unid).await?;
            let Some(activity) = activity_from_page(&document) else {
                continue;
            };
            if !activity.is_attendance() {
                continue;
            }
            if activity.open {
                debug!(unid = %activity.unid, "found an open attendance activity");
                return Ok(activity);
            }
            fallback.get_or_insert(activity);
        }

        fallback.ok_or_else(|| {
            AppError::Parse(format!("no attendance activity found on {activities_url}"))
        })
    }

    /// Open an activity document within a course database.
    ///
    /// # Errors
    /// Propagates transport failures.
    pub(crate) async fn open_activity(
        &mut self,
        activities_url: &str,
        unid: &str,
    ) -> Result<String> {
        let database = activities_url
            .split(".nsf")
            .next()
            .ok_or_else(|| AppError::Parse(format!("no database in {activities_url}")))?
            .to_owned();
        let url = format!("{database}.nsf/0/{unid}?OpenDocument");
        self.session.get(&url).await
    }

    /// Submit attendance for an activity using the given coordinates.
    ///
    /// # Errors
    /// Propagates transport failures.
    pub async fn submit_attendance(
        &mut self,
        activities_url: &str,
        unid: &str,
        coordinates: Option<(f64, f64)>,
    ) -> Result<AttendanceOutcome> {
        let (lat, lng) = coordinates.unwrap_or((0.0, 0.0));
        let url = format!("{activities_url}?createdocument&puid={unid}&lat={lat}&lng={lng}");
        let body = self.session.get(&url).await?;
        let outcome = AttendanceOutcome::classify(&body);
        info!(?outcome, "attendance submission answered");
        Ok(outcome)
    }
}

/// A task-list entry rendered on a class-activities page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskLink {
    /// Domino document identifier carried by the link's `href`.
    pub unid: String,
    /// `classid` attribute, e.g. `L01-20260921-11:00-N`.
    pub class_id: String,
}

/// Build the `classid` prefix that identifies this class's task entry.
///
/// The page encodes `{group}-{YYYYMMDD}-{HH:MM}` followed by an activity-type
/// suffix such as `-N`, so matching on this prefix selects the intended
/// session without depending on the suffix.
pub(crate) fn expected_class_id(class: &ScheduledClass) -> String {
    format!(
        "{}-{}-{}",
        class.group,
        class.starts_at.strftime("%Y%m%d"),
        class.starts_at.strftime("%H:%M")
    )
}

/// Collect every task link on a class-activities page.
pub(crate) fn task_links(page: &str) -> Vec<TaskLink> {
    let mut links = Vec::new();
    let mut cursor = 0;

    while let Some(found) = page.get(cursor..).and_then(|rest| rest.find("href='")) {
        let Some(href_at) = cursor.checked_add(found) else {
            break;
        };
        let Some(value_start) = href_at.checked_add(6) else {
            break;
        };
        let Some(remainder) = page.get(value_start..) else {
            break;
        };
        let Some(value_end) = remainder.find('\'') else {
            break;
        };
        let unid = remainder.get(..value_end).unwrap_or_default();

        let tag_start = page
            .get(..href_at)
            .and_then(|head| head.rfind('<'))
            .unwrap_or(0);
        let tag = page
            .get(tag_start..)
            .and_then(|rest| rest.find('>').map(|end| rest.get(..end)))
            .flatten()
            .unwrap_or_default();
        let class_id = html::attribute_value(tag, "classid").unwrap_or_default();

        if unid.len() == 32 && unid.chars().all(|c| c.is_ascii_hexdigit()) {
            links.push(TaskLink {
                unid: unid.to_owned(),
                class_id,
            });
        }
        cursor = value_start.saturating_add(value_end).saturating_add(1);
    }

    links
}

/// Whether an activity document reports a running window.
fn activity_is_open(document: &str) -> bool {
    let remaining = html::js_var(document, "time_left_second")
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(0);
    let starts_in = html::js_var(document, "time_to_start_second")
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(0);
    remaining > 0 && starts_in <= 0
}

/// Extract an activity directly embedded in a page.
fn activity_from_page(page: &str) -> Option<Activity> {
    let unid = html::js_var(page, "unid")?;
    if unid.is_empty() {
        return None;
    }
    Some(Activity {
        unid,
        attendance_type: html::js_var(page, "attendance_type").unwrap_or_default(),
        open: activity_is_open(page),
    })
}

#[cfg(test)]
mod tests {
    use super::{Activity, activity_from_page, expected_class_id, task_links};
    use crate::domain::schedule::ScheduledClass;

    #[test]
    fn reads_an_inline_activity_from_the_list_page() {
        // Given a rendered activity page carrying both variables.
        let page = r#"<script>var attendance_type="2"; var unid="ABC123";</script>"#;

        // When an activity is extracted.
        let activity = activity_from_page(page).expect("activity is present");

        // Then both fields are captured and the type is recognised.
        assert_eq!(activity.unid, "ABC123");
        assert!(activity.is_attendance());
    }

    #[test]
    fn finds_no_activity_on_the_task_list_page() {
        // Given the list page, which declares unid as empty.
        let page = r#"<script>var attendance_type=""; var unid="";</script>"#;

        // When an activity is extracted.
        // Then absence is reported rather than an empty activity.
        assert!(activity_from_page(page).is_none());
    }

    #[test]
    fn collects_task_links_with_their_class_ids() {
        // Given the task list markup the list page renders.
        let page = r#"<ul id="mytask_list">
            <li><a start='2026-09-21T02:57:40Z' href='356D6AE68EED315148258E7900104316' classid='L01-20260921-11:00-N'>QR Code</a></li>
            <li><a href='91CFC9EDB617604E48258E79001A994F' classid='T01-20260921-13:00-N'>QR Code</a></li>
        </ul>"#;

        // When the task list is parsed.
        let tasks = task_links(page);

        // Then both entries carry their document id and class id.
        assert_eq!(tasks.len(), 2);
        assert_eq!(
            tasks.first().expect("first task").unid,
            "356D6AE68EED315148258E7900104316"
        );
        assert_eq!(
            tasks.first().expect("first task").class_id,
            "L01-20260921-11:00-N"
        );
        assert_eq!(
            tasks.get(1).expect("second task").class_id,
            "T01-20260921-13:00-N"
        );
    }

    #[test]
    fn ignores_task_links_that_are_not_document_ids() {
        // Given a list whose links are not document identifiers.
        let page = r"<li><a href='javascript:refresh_page()'>Refresh</a></li>";

        // When the task list is parsed.
        // Then nothing is collected.
        assert!(task_links(page).is_empty());
    }

    #[test]
    fn derives_the_class_id_matching_the_live_markup() {
        // Given the class observed on the live timetable.
        let class_info = ScheduledClass {
            termcode: "2604".to_owned(),
            course_code: "ELEC3050SEF".to_owned(),
            name: "Lecture ( Full Time )".to_owned(),
            starts_at: jiff::civil::date(2026, 9, 21).at(11, 0, 0, 0),
            ends_at: None,
            group: "L01".to_owned(),
            venue: "HKMU C0G01".to_owned(),
        };

        // When the expected class id is built.
        // Then it is a prefix of the live markup (which appends an activity-type
        // suffix), so the right session is chosen instead of whichever task
        // happened to be listed first.
        assert_eq!(expected_class_id(&class_info), "L01-20260921-11:00");
        assert!("L01-20260921-11:00-N".starts_with(&expected_class_id(&class_info)));
    }

    #[test]
    fn distinguishes_two_sessions_of_the_same_course() {
        // Given the lecture and tutorial of one course on the same day.
        let base = ScheduledClass {
            termcode: "2604".to_owned(),
            course_code: "ELEC3050SEF".to_owned(),
            name: String::new(),
            starts_at: jiff::civil::date(2026, 9, 21).at(11, 0, 0, 0),
            ends_at: None,
            group: "L01".to_owned(),
            venue: String::new(),
        };
        let mut tutorial = base.clone();
        tutorial.group = "T01".to_owned();
        tutorial.starts_at = jiff::civil::date(2026, 9, 21).at(13, 0, 0, 0);

        // When both class ids are produced.
        // Then they differ, so each session targets its own activity.
        assert_ne!(expected_class_id(&base), expected_class_id(&tutorial));
        assert_eq!(expected_class_id(&tutorial), "T01-20260921-13:00");
        assert!("T01-20260921-13:00-N".starts_with(&expected_class_id(&tutorial)));
    }

    #[test]
    fn defaults_coordinates_to_origin() {
        // Given an activity type and no geolocation available.
        let activity = Activity {
            unid: "ABC".to_owned(),
            attendance_type: "2".to_owned(),
            open: true,
        };

        // When the attendance type is checked.
        // Then it is recognised as the geolocation-backed attendance activity.
        assert!(activity.is_attendance());
    }
}
