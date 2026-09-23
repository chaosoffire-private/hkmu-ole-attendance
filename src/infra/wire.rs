//! The `getTodayClass` wire format, and its translation into the domain.
//!
//! These types exist to decode one JSON envelope; they are not the domain's
//! vocabulary, so they live here and are converted at the boundary. Keeping
//! them out of `domain` means a change to the API's field names or result
//! encoding cannot reach the business rules.

use serde::Deserialize;

use crate::domain::schedule::{Course, Outcome, Session, Timetable};

/// `result` in OLE payloads is sometimes a number and sometimes a string.
///
/// The API answers `"result": 1` on success and `"result": "-1"` together with
/// an `error` field on rejection, so both shapes must be accepted.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum WireResult {
    Number(i64),
    Text(String),
}

impl WireResult {
    /// Whether the value is the documented success code (`1`).
    fn is_success(&self) -> bool {
        match self {
            Self::Number(value) => *value == 1,
            Self::Text(value) => value.trim() == "1",
        }
    }
}

/// Top-level `getTodayClass` response, exactly as the API sends it.
#[derive(Debug, Deserialize)]
pub struct WireResponse {
    result: WireResult,
    #[serde(default)]
    classes: Vec<WireCourse>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    errormsg: Option<String>,
}

/// One course in the payload.
#[derive(Debug, Deserialize)]
struct WireCourse {
    #[serde(default)]
    termcode: String,
    #[serde(default)]
    course_code: String,
    #[serde(default)]
    classes: Vec<WireSession>,
}

/// One session in the payload.
#[derive(Debug, Deserialize)]
struct WireSession {
    #[serde(default)]
    name: String,
    #[serde(default)]
    datetime: String,
    #[serde(default)]
    endtime: String,
    #[serde(default)]
    group: String,
    #[serde(default)]
    venue: String,
}

impl From<WireResponse> for Timetable {
    fn from(wire: WireResponse) -> Self {
        let WireResponse {
            result,
            classes,
            error,
            errormsg,
        } = wire;

        let outcome = if result.is_success() {
            Outcome::Success
        } else {
            Outcome::Rejected {
                reason: summarise(error, errormsg),
            }
        };

        let courses = classes
            .into_iter()
            .map(|course| Course {
                termcode: course.termcode,
                course_code: course.course_code,
                sessions: course
                    .classes
                    .into_iter()
                    .map(|session| Session {
                        name: session.name,
                        datetime: session.datetime,
                        endtime: session.endtime,
                        group: session.group,
                        venue: session.venue,
                    })
                    .collect(),
            })
            .collect();

        Self { outcome, courses }
    }
}

/// The API's own description of a rejection, combining its code and message.
fn summarise(error: Option<String>, errormsg: Option<String>) -> Option<String> {
    match (error, errormsg) {
        (Some(code), Some(msg)) => Some(format!("{code}: {msg}")),
        (Some(code), None) => Some(code),
        (None, Some(msg)) => Some(msg),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::WireResponse;
    use crate::domain::schedule::{Outcome, Timetable};

    fn decode(json: &str) -> Timetable {
        let wire: WireResponse = serde_json::from_str(json).expect("test payload is valid");
        wire.into()
    }

    #[test]
    fn decodes_the_numeric_success_payload() {
        // Given the live success payload.
        let timetable = decode(
            r#"{"result":1,"classes":[{"termcode":"2604","course_code":"COMP4930SEF","classes":[
                {"name":"PC Laboratory ( Full Time )","datetime":"2026-09-21 09:00","endtime":"2026-09-21 09:50","group":"P02","venue":"MUPC C0412","host":"kxma"}]}],"is_cc":false}"#,
        );

        // When inspected.
        // Then success is recognised and the fields survive the translation.
        assert_eq!(timetable.outcome, Outcome::Success);
        assert!(timetable.is_success());
        assert_eq!(timetable.courses.len(), 1);
        assert_eq!(
            timetable.courses.first().expect("course").course_code,
            "COMP4930SEF"
        );
    }

    #[test]
    fn decodes_the_string_error_payload() {
        // Given the live unauthenticated payload with a string result.
        let timetable =
            decode(r#"{"result":"-1","error":"9901","errormsg":"The API is only for web users."}"#);

        // When inspected.
        // Then it is not success and the API error is surfaced verbatim.
        assert!(!timetable.is_success());
        assert_eq!(
            timetable.outcome,
            Outcome::Rejected {
                reason: Some("9901: The API is only for web users.".to_owned())
            }
        );
    }

    #[test]
    fn tolerates_unknown_extra_fields() {
        // Given a payload carrying a field this build does not model.
        // When decoded.
        // Then decoding succeeds, so an upstream addition cannot break us.
        let timetable = decode(r#"{"result":1,"classes":[],"is_cc":false,"future_field":[1,2]}"#);
        assert!(timetable.is_success());
    }
}
