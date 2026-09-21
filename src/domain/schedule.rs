//! Timetable values and the rules that select which sessions matter today.
//!
//! Pure: no HTTP, no clock, no storage. The day being filtered is supplied by
//! the caller.

use jiff::civil::Date;
use serde::{Deserialize, Serialize};

use super::error::DomainError;
use super::time::parse_class_time;

/// `result` in OLE payloads is sometimes a number and sometimes a string.
///
/// The API answers `"result": 1` on success and `"result": "-1"` together with
/// an `error` field on rejection, so both shapes must be accepted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ApiResult {
    /// Numeric form, e.g. `1`.
    Number(i64),
    /// String form, e.g. `"-1"`.
    Text(String),
}

impl ApiResult {
    /// Whether the value is the documented success code (`1`).
    pub fn is_success(&self) -> bool {
        match self {
            Self::Number(value) => *value == 1,
            Self::Text(value) => value.trim() == "1",
        }
    }
}

/// Top-level `getTodayClass` response, exactly as the API sends it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodayClassResponse {
    /// Success indicator.
    pub result: ApiResult,
    /// Courses with their sessions; absent on error payloads.
    #[serde(default)]
    pub classes: Vec<Course>,
    /// API error code, present on rejection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Human-readable API error, present on rejection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub errormsg: Option<String>,
}

impl TodayClassResponse {
    /// Whether the payload carries a successful class list.
    pub fn is_success(&self) -> bool {
        self.result.is_success()
    }

    /// The API's own error description, if any.
    pub fn error_summary(&self) -> Option<String> {
        match (&self.error, &self.errormsg) {
            (Some(code), Some(msg)) => Some(format!("{code}: {msg}")),
            (Some(code), None) => Some(code.clone()),
            (None, Some(msg)) => Some(msg.clone()),
            (None, None) => None,
        }
    }
}

/// One course offered in the current term.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Course {
    /// Term identifier, e.g. `2604`.
    #[serde(default)]
    pub termcode: String,
    /// Course code, e.g. `ELEC3050SEF`.
    #[serde(default)]
    pub course_code: String,
    /// Sessions belonging to this course.
    #[serde(default)]
    pub classes: Vec<ClassSession>,
}

/// A single scheduled session, as the API describes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassSession {
    /// Display name, e.g. `Lecture ( Full Time )`.
    #[serde(default)]
    pub name: String,
    /// Start time as `YYYY-MM-DD HH:MM` in local wall-clock time.
    #[serde(default)]
    pub datetime: String,
    /// End time as `YYYY-MM-DD HH:MM`, or empty when unknown.
    #[serde(default)]
    pub endtime: String,
    /// Group code, e.g. `L01`.
    #[serde(default)]
    pub group: String,
    /// Room or venue.
    #[serde(default)]
    pub venue: String,
    /// Hosting staff account.
    #[serde(default)]
    pub host: String,
}

/// A session resolved into typed times and a concrete attendance URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledClass {
    /// Term identifier.
    pub termcode: String,
    /// Course code.
    pub course_code: String,
    /// Display name.
    pub name: String,
    /// Local start time.
    pub starts_at: jiff::civil::DateTime,
    /// Local end time, when the API supplied one.
    pub ends_at: Option<jiff::civil::DateTime>,
    /// Group code.
    pub group: String,
    /// Room or venue.
    pub venue: String,
}

impl ScheduledClass {
    /// Stable one-line description used in logs and notifications.
    pub fn label(&self) -> String {
        format!("{} - {}", self.course_code, self.name)
    }
}

/// The timetable for one day, after filtering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaySchedule {
    /// Success indicator, carried through from the API.
    pub result: ApiResult,
    /// Sessions occurring on the selected day.
    pub classes: Vec<ScheduledClass>,
}

/// Select the sessions that occur on `day`, resolving their times.
///
/// A session whose times cannot be parsed is dropped, because acting on an
/// uninterpretable time could mean submitting attendance at the wrong moment.
pub fn select_day(payload: &TodayClassResponse, day: Date) -> DaySchedule {
    let mut classes = Vec::new();

    for course in &payload.classes {
        if course.termcode.is_empty() || course.course_code.is_empty() {
            continue;
        }
        for session in &course.classes {
            let Ok(started) = parse_class_time(&session.datetime) else {
                continue;
            };
            if started.date() != day {
                continue;
            }
            let ends_at = if session.endtime.trim().is_empty() {
                None
            } else {
                parse_class_time(&session.endtime).ok()
            };
            classes.push(ScheduledClass {
                termcode: course.termcode.clone(),
                course_code: course.course_code.clone(),
                name: session.name.clone(),
                starts_at: started,
                ends_at,
                group: session.group.clone(),
                venue: session.venue.clone(),
            });
        }
    }

    classes.sort_by_key(|class| class.starts_at);

    DaySchedule {
        result: payload.result.clone(),
        classes,
    }
}

/// The class-activities page for a course.
///
/// # Errors
/// [`DomainError::Url`] is not returned today, but the signature stays fallible
/// so a future move to real URL validation does not change call sites.
pub fn activities_url(
    ole_url: &str,
    termcode: &str,
    course_code: &str,
) -> Result<String, DomainError> {
    let base = ole_url.trim_end_matches('/');
    Ok(format!(
        "{base}/course{termcode}/{course_code}.nsf//class_activities_student?readform&"
    ))
}

/// Build the "retrieved classes" notification text.
pub fn format_classes_message(schedule: &DaySchedule, system_time: &str) -> String {
    if !schedule.result.is_success() {
        return "Failed to retrieve class information".to_owned();
    }
    if schedule.classes.is_empty() {
        return "No classes scheduled for today!".to_owned();
    }

    let mut message = String::from("**Retrieved Classes:**\n");
    message.push_str("`System Time: ");
    message.push_str(system_time);
    message.push_str("`\n\n");

    for class in &schedule.classes {
        let start = class.starts_at.strftime("%H:%M").to_string();
        let time_range = match class.ends_at {
            Some(end) => format!("{start} - {}", end.strftime("%H:%M")),
            None => start,
        };
        message.push_str("> **");
        message.push_str(&class.course_code);
        message.push_str("** - ");
        message.push_str(&class.name);
        message.push_str("\n>  TIME: ");
        message.push_str(&time_range);
        message.push_str("\n>  VENUE: ");
        message.push_str(&class.venue);
        if !class.group.is_empty() {
            message.push_str(" (");
            message.push_str(&class.group);
            message.push(')');
        }
        message.push('\n');
    }
    message
}

#[cfg(test)]
mod tests {
    use jiff::civil::date;

    use super::{TodayClassResponse, activities_url, format_classes_message, select_day};

    fn payload(json: &str) -> TodayClassResponse {
        serde_json::from_str(json).expect("test payload is valid")
    }

    #[test]
    fn parses_the_numeric_success_payload() {
        // Given the live success payload.
        let source = payload(
            r#"{"result":1,"classes":[{"termcode":"2604","course_code":"COMP4930SEF","classes":[
                {"name":"PC Laboratory ( Full Time )","datetime":"2026-09-21 09:00","endtime":"2026-09-21 09:50","group":"P02","venue":"MUPC C0412","host":"kxma"}]}],"is_cc":false}"#,
        );

        // When inspected.
        // Then success is recognised and fields survive intact.
        assert!(source.is_success());
        assert_eq!(source.classes.len(), 1);
        assert_eq!(
            source.classes.first().expect("course").course_code,
            "COMP4930SEF"
        );
        assert!(source.error_summary().is_none());
    }

    #[test]
    fn parses_the_string_error_payload() {
        // Given the live unauthenticated payload with a string result.
        let source = payload(
            r#"{"result":"-1","error":"9901","errormsg":"The API is only for web users."}"#,
        );

        // When inspected.
        // Then it is not success and the API error is surfaced verbatim.
        assert!(!source.is_success());
        assert_eq!(
            source.error_summary().as_deref(),
            Some("9901: The API is only for web users.")
        );
    }

    #[test]
    fn tolerates_unknown_extra_fields() {
        // Given a payload carrying a field this build does not model.
        // When decoded.
        let source = payload(r#"{"result":1,"classes":[],"is_cc":false,"future_field":[1,2]}"#);

        // Then decoding succeeds, so an upstream addition cannot break us.
        assert!(source.is_success());
    }

    #[test]
    fn keeps_only_sessions_on_the_target_day() {
        // Given a timetable spanning two days.
        let source = payload(
            r#"{"result":1,"classes":[{"termcode":"2604","course_code":"A","classes":[
                {"name":"Today","datetime":"2026-09-21 09:00","endtime":"2026-09-21 10:00","group":"L01","venue":"R1"},
                {"name":"Tomorrow","datetime":"2026-09-22 09:00","endtime":"2026-09-22 10:00","group":"L01","venue":"R1"}]}]}"#,
        );

        // When filtered to 2026-09-21.
        let schedule = select_day(&source, date(2026, 9, 21));

        // Then only the matching session survives.
        assert_eq!(schedule.classes.len(), 1);
        assert_eq!(schedule.classes.first().expect("class").name, "Today");
    }

    #[test]
    fn drops_a_session_whose_time_cannot_be_parsed() {
        // Given a course mixing a valid session with an unparseable one.
        let source = payload(
            r#"{"result":1,"classes":[{"termcode":"2604","course_code":"A","classes":[
                {"name":"Broken","datetime":"whenever","venue":"R"},
                {"name":"Valid","datetime":"2026-09-21 09:00","venue":"R"}]}]}"#,
        );

        // When filtered.
        let schedule = select_day(&source, date(2026, 9, 21));

        // Then only the interpretable session is acted upon.
        assert_eq!(schedule.classes.len(), 1);
        assert_eq!(schedule.classes.first().expect("class").name, "Valid");
    }

    #[test]
    fn orders_sessions_by_start_time() {
        // Given sessions listed out of chronological order.
        let source = payload(
            r#"{"result":1,"classes":[{"termcode":"2604","course_code":"A","classes":[
                {"name":"Late","datetime":"2026-09-21 15:00","venue":"R"},
                {"name":"Early","datetime":"2026-09-21 09:00","venue":"R"}]}]}"#,
        );

        // When filtered.
        let schedule = select_day(&source, date(2026, 9, 21));

        // Then they are ordered, so scheduling follows the day.
        let names: Vec<&str> = schedule.classes.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["Early", "Late"]);
    }

    #[test]
    fn builds_the_activities_url() {
        // Given the portal base and a course.
        // When the URL is built.
        let url =
            activities_url("https://iole.hkmu.edu.hk", "2604", "ELEC3050SEF").expect("builds");

        // Then it matches the path the site serves.
        assert_eq!(
            url,
            "https://iole.hkmu.edu.hk/course2604/ELEC3050SEF.nsf//class_activities_student?readform&"
        );
    }

    #[test]
    fn reports_failure_and_empty_states_verbatim() {
        // Given an unsuccessful payload and an empty successful one.
        let failed = select_day(
            &payload(r#"{"result":"-1","error":"9901","errormsg":"nope"}"#),
            date(2026, 9, 21),
        );
        let empty = select_day(&payload(r#"{"result":1,"classes":[]}"#), date(2026, 9, 21));

        // When rendered.
        // Then the strings match the previous implementation exactly.
        assert_eq!(
            format_classes_message(&failed, "T"),
            "Failed to retrieve class information"
        );
        assert_eq!(
            format_classes_message(&empty, "T"),
            "No classes scheduled for today!"
        );
    }

    #[test]
    fn renders_the_class_list_message() {
        // Given one session today.
        let source = payload(
            r#"{"result":1,"classes":[{"termcode":"2604","course_code":"COMP4930SEF","classes":[
                {"name":"PC Laboratory ( Full Time )","datetime":"2026-09-21 09:00","endtime":"2026-09-21 09:50","group":"P02","venue":"MUPC C0412"}]}]}"#,
        );

        // When rendered.
        let message = format_classes_message(&select_day(&source, date(2026, 9, 21)), "S");

        // Then the header and the per-session block match the documented format.
        assert!(message.starts_with("**Retrieved Classes:**\n`System Time: S`"));
        assert!(message.contains("> **COMP4930SEF** - PC Laboratory ( Full Time )"));
        assert!(message.contains(">  TIME: 09:00 - 09:50"));
        assert!(message.contains(">  VENUE: MUPC C0412 (P02)"));
    }

    #[test]
    fn renders_a_start_time_only_when_the_end_is_missing() {
        // Given a session with no end time.
        let source = payload(
            r#"{"result":1,"classes":[{"termcode":"2604","course_code":"A","classes":[
                {"name":"Open","datetime":"2026-09-21 09:00","venue":"R"}]}]}"#,
        );

        // When rendered.
        let message = format_classes_message(&select_day(&source, date(2026, 9, 21)), "S");

        // Then only the start time appears, with no dangling separator.
        assert!(message.contains(">  TIME: 09:00\n"));
    }
}
