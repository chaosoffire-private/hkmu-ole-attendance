//! Timetable values and the rules that select which sessions matter today.
//!
//! Pure: no HTTP, no clock, no storage, and no knowledge of the API's JSON
//! shape. The wire format is decoded in `infra::wire` and translated into these
//! types at the boundary.

use jiff::civil::Date;

use super::time::parse_class_time;

/// Whether the service handed back a usable timetable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The service answered successfully.
    Success,
    /// The service refused, optionally saying why.
    Rejected {
        /// The service's own description of the refusal, when it gave one.
        reason: Option<String>,
    },
}

impl Outcome {
    /// Whether the service answered successfully.
    pub const fn is_success(&self) -> bool {
        matches!(self, Self::Success)
    }

    /// The service's own description of a refusal, if any.
    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Success => None,
            Self::Rejected { reason } => reason.as_deref(),
        }
    }
}

/// One course offered in the current term.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Course {
    /// Term identifier, e.g. `2604`.
    pub termcode: String,
    /// Course code, e.g. `ELEC3050SEF`.
    pub course_code: String,
    /// Sessions belonging to this course.
    pub sessions: Vec<Session>,
}

/// A single scheduled session as the service describes it, before its times are
/// interpreted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    /// Display name, e.g. `Lecture ( Full Time )`.
    pub name: String,
    /// Start time as `YYYY-MM-DD HH:MM` in local wall-clock time.
    pub datetime: String,
    /// End time as `YYYY-MM-DD HH:MM`, or empty when unknown.
    pub endtime: String,
    /// Group code, e.g. `L01`.
    pub group: String,
    /// Room or venue.
    pub venue: String,
}

/// The timetable the service reported, with its courses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Timetable {
    /// Whether the service answered successfully.
    pub outcome: Outcome,
    /// Courses with their sessions; empty on a refusal.
    pub courses: Vec<Course>,
}

impl Timetable {
    /// Whether the timetable carries a successful course list.
    pub const fn is_success(&self) -> bool {
        self.outcome.is_success()
    }

    /// The service's own description of a refusal, if any.
    pub fn error_summary(&self) -> Option<String> {
        self.outcome.reason().map(str::to_owned)
    }
}

/// A session resolved into typed times.
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
    /// Local end time, when the service supplied one.
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
    /// Whether the service answered successfully.
    pub outcome: Outcome,
    /// Sessions occurring on the selected day.
    pub classes: Vec<ScheduledClass>,
}

impl DaySchedule {
    /// Whether the service answered successfully.
    pub const fn is_success(&self) -> bool {
        self.outcome.is_success()
    }
}

/// Select the sessions that occur on `day`, resolving their times.
///
/// A session whose times cannot be parsed is dropped, because acting on an
/// uninterpretable time could mean submitting attendance at the wrong moment.
pub fn select_day(timetable: &Timetable, day: Date) -> DaySchedule {
    let mut classes = Vec::new();

    for course in &timetable.courses {
        if course.termcode.is_empty() || course.course_code.is_empty() {
            continue;
        }
        for session in &course.sessions {
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
        outcome: timetable.outcome.clone(),
        classes,
    }
}

/// Build the "retrieved classes" notification text.
pub fn format_classes_message(schedule: &DaySchedule, system_time: &str) -> String {
    if !schedule.is_success() {
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

    use super::{
        Course, DaySchedule, Outcome, Session, Timetable, format_classes_message, select_day,
    };

    /// A timetable whose single course carries `sessions`.
    fn timetable(sessions: Vec<Session>) -> Timetable {
        Timetable {
            outcome: Outcome::Success,
            courses: vec![Course {
                termcode: "2604".to_owned(),
                course_code: "A".to_owned(),
                sessions,
            }],
        }
    }

    fn session(name: &str, datetime: &str, endtime: &str) -> Session {
        Session {
            name: name.to_owned(),
            datetime: datetime.to_owned(),
            endtime: endtime.to_owned(),
            group: "L01".to_owned(),
            venue: "R1".to_owned(),
        }
    }

    #[test]
    fn keeps_only_sessions_on_the_target_day() {
        // Given a timetable spanning two days.
        let source = timetable(vec![
            session("Today", "2026-09-21 09:00", "2026-09-21 10:00"),
            session("Tomorrow", "2026-09-22 09:00", "2026-09-22 10:00"),
        ]);

        // When filtered to 2026-09-21.
        let schedule = select_day(&source, date(2026, 9, 21));

        // Then only the matching session survives.
        assert_eq!(schedule.classes.len(), 1);
        assert_eq!(schedule.classes.first().expect("class").name, "Today");
    }

    #[test]
    fn drops_a_session_whose_time_cannot_be_parsed() {
        // Given a course mixing a valid session with an unparseable one.
        let source = timetable(vec![
            session("Broken", "whenever", ""),
            session("Valid", "2026-09-21 09:00", ""),
        ]);

        // When filtered.
        let schedule = select_day(&source, date(2026, 9, 21));

        // Then only the interpretable session is acted upon.
        assert_eq!(schedule.classes.len(), 1);
        assert_eq!(schedule.classes.first().expect("class").name, "Valid");
    }

    #[test]
    fn orders_sessions_by_start_time() {
        // Given sessions listed out of chronological order.
        let source = timetable(vec![
            session("Late", "2026-09-21 15:00", ""),
            session("Early", "2026-09-21 09:00", ""),
        ]);

        // When filtered.
        let schedule = select_day(&source, date(2026, 9, 21));

        // Then they are ordered, so scheduling follows the day.
        let names: Vec<&str> = schedule.classes.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["Early", "Late"]);
    }

    #[test]
    fn skips_a_course_missing_its_identifiers() {
        // Given a course with no term code, which cannot build a URL.
        let source = Timetable {
            outcome: Outcome::Success,
            courses: vec![
                Course {
                    termcode: String::new(),
                    course_code: "A".to_owned(),
                    sessions: vec![session("Orphan", "2026-09-21 09:00", "")],
                },
                Course {
                    termcode: "2604".to_owned(),
                    course_code: "B".to_owned(),
                    sessions: vec![session("Kept", "2026-09-21 10:00", "")],
                },
            ],
        };

        // When filtered.
        let schedule = select_day(&source, date(2026, 9, 21));

        // Then only the identifiable course contributes.
        assert_eq!(schedule.classes.len(), 1);
        assert_eq!(schedule.classes.first().expect("class").course_code, "B");
    }

    #[test]
    fn reports_the_refusal_reason() {
        // Given a rejected timetable carrying a reason.
        let rejected = Timetable {
            outcome: Outcome::Rejected {
                reason: Some("9901: nope".to_owned()),
            },
            courses: Vec::new(),
        };

        // When inspected.
        // Then it is not success and the reason is surfaced.
        assert!(!rejected.is_success());
        assert_eq!(rejected.error_summary().as_deref(), Some("9901: nope"));
    }

    #[test]
    fn reports_failure_and_empty_states_verbatim() {
        // Given an unsuccessful and an empty successful schedule.
        let failed = select_day(
            &Timetable {
                outcome: Outcome::Rejected { reason: None },
                courses: Vec::new(),
            },
            date(2026, 9, 21),
        );
        let empty = select_day(&timetable(Vec::new()), date(2026, 9, 21));

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
        let source = timetable(vec![Session {
            name: "PC Laboratory ( Full Time )".to_owned(),
            datetime: "2026-09-21 09:00".to_owned(),
            endtime: "2026-09-21 09:50".to_owned(),
            group: "P02".to_owned(),
            venue: "MUPC C0412".to_owned(),
        }]);

        // When rendered.
        let message = format_classes_message(&select_day(&source, date(2026, 9, 21)), "S");

        // Then the header and the per-session block match the documented format.
        assert!(message.starts_with("**Retrieved Classes:**\n`System Time: S`"));
        assert!(message.contains("> **A** - PC Laboratory ( Full Time )"));
        assert!(message.contains(">  TIME: 09:00 - 09:50"));
        assert!(message.contains(">  VENUE: MUPC C0412 (P02)"));
    }

    #[test]
    fn renders_a_start_time_only_when_the_end_is_missing() {
        // Given a session with no end time.
        let source = timetable(vec![session("Open", "2026-09-21 09:00", "")]);

        // When rendered.
        let message = format_classes_message(&select_day(&source, date(2026, 9, 21)), "S");

        // Then only the start time appears, with no dangling separator.
        assert!(message.contains(">  TIME: 09:00\n"));
    }

    #[test]
    fn an_empty_schedule_is_still_successful() {
        // Given a successful timetable with no courses.
        let schedule: DaySchedule = select_day(&timetable(Vec::new()), date(2026, 9, 21));

        // Then it is a success with no classes, not a failure.
        assert!(schedule.is_success());
        assert!(schedule.classes.is_empty());
    }
}
