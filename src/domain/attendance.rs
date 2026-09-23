//! The attendance polling policy, expressed as a pure state machine.
//!
//! This is the decision core that previously lived inside an `async fn` bound
//! to concrete HTTP, cache and notification types, which made it impossible to
//! unit test. Here every decision is a function of values supplied by the
//! caller: the current instant, the class window, and the last observed
//! submission outcome.

use std::time::Duration;

use jiff::Zoned;
use jiff::tz::TimeZone;

use super::schedule::ScheduledClass;
use super::time::{class_end, minutes_until};

/// What the poller should do next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// The class has not started; wait this long, then re-evaluate.
    Wait(Duration),
    /// Submit attendance now.
    Poll,
    /// The class is over; stop.
    Finished,
}

/// Whether the submission window is open, as reported by the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Submission {
    /// Attendance is recorded.
    Confirmed,
    /// The window has closed.
    Closed,
    /// The activity is live but nothing was recorded yet.
    NotYet,
    /// The server answered with something unrecognised.
    Unknown(String),
}

impl Submission {
    /// Classify a raw submission response body.
    ///
    /// The server answers either with a bare status or with an envelope of the
    /// form `success|<datetime>|<status>|...`, so the status is matched as a
    /// whole field. Substring matching must not be used here: `contains("active")`
    /// also matches "inactive" and `contains("present")` also matches
    /// "represent", and either would report attendance that was never recorded —
    /// the one failure this program must never make, since a confirmation stops
    /// polling and tells the operator the class is safe.
    ///
    /// The `success` token is only the envelope marker, not a success test, so
    /// `success|...|Absent|...` must not be read as confirmed.
    pub fn classify(body: &str) -> Self {
        const CLOSED: &str = "submission closed";
        const ENVELOPE: &str = "success";

        let lower = body.to_ascii_lowercase();
        // Closed wins outright: a late poll must never read as a confirmation.
        if lower.contains(CLOSED) {
            return Self::Closed;
        }
        // Either the envelope's status field, or a bare status body.
        let recorded =
            status_field(&lower).is_some_and(is_recorded_status) || is_recorded_status(&lower);
        if recorded {
            return Self::Confirmed;
        }
        if lower.contains(ENVELOPE) || lower.contains('|') {
            return Self::NotYet;
        }
        Self::Unknown(body.trim().chars().take(120).collect())
    }

    /// Whether attendance is securely recorded.
    pub const fn is_confirmed(&self) -> bool {
        matches!(self, Self::Confirmed)
    }
}

/// The `<status>` field of a `success|<datetime>|<status>|...` envelope.
fn status_field(lower: &str) -> Option<&str> {
    let mut fields = lower.split('|');
    let _envelope = fields.next()?;
    let _datetime = fields.next()?;
    fields.next().map(str::trim)
}

/// Whether a whole status token means attendance was recorded.
fn is_recorded_status(token: &str) -> bool {
    let token = token.trim();
    token == "present" || token == "active"
}

impl std::fmt::Display for Submission {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Confirmed => f.write_str("confirmed"),
            Self::Closed => f.write_str("submission window closed"),
            Self::NotYet => f.write_str("not yet recorded"),
            Self::Unknown(body) => write!(f, "unrecognised response: {body}"),
        }
    }
}

/// A class window resolved to instants, ready for decision making.
#[derive(Debug, Clone, Copy)]
pub struct ClassWindow {
    /// When the class starts.
    pub starts_at: jiff::Timestamp,
    /// When the class ends.
    pub ends_at: jiff::Timestamp,
}

impl ClassWindow {
    /// Resolve a scheduled class into instants.
    ///
    /// Returns `None` when the civil times cannot be placed in `zone`.
    pub fn resolve(class: &ScheduledClass, fallback: Duration, zone: &TimeZone) -> Option<Self> {
        let ends = class_end(class.starts_at, class.ends_at, fallback, zone)?;
        let starts = class.starts_at.to_zoned(zone.clone()).ok()?;
        Some(Self {
            starts_at: starts.timestamp(),
            ends_at: ends.timestamp(),
        })
    }

    /// Whether the class has already finished at `now`.
    pub fn is_over(&self, now: jiff::Timestamp) -> bool {
        now.as_second() >= self.ends_at.as_second()
    }

    /// Whole minutes elapsed since the start, saturating at zero.
    pub fn minutes_since_start(&self, now: jiff::Timestamp) -> i64 {
        now.as_second()
            .saturating_sub(self.starts_at.as_second())
            .max(0)
            .checked_div(60)
            .unwrap_or(0)
    }

    /// Whole minutes elapsed since the end, saturating at zero.
    pub fn minutes_since_end(&self, now: jiff::Timestamp) -> i64 {
        now.as_second()
            .saturating_sub(self.ends_at.as_second())
            .max(0)
            .checked_div(60)
            .unwrap_or(0)
    }
}

/// Decide what the poller should do now.
///
/// This is the single place the poll loop consults: `Finished` covers the class
/// having ended, so the caller does not re-test the window and risk disagreeing
/// with this decision.
pub fn next_action(window: &ClassWindow, now: jiff::Timestamp) -> Action {
    if window.is_over(now) {
        return Action::Finished;
    }
    let until_start = window.starts_at.as_second().saturating_sub(now.as_second());
    if until_start > 0 {
        return Action::Wait(Duration::from_secs(u64::try_from(until_start).unwrap_or(0)));
    }
    Action::Poll
}

/// Whether the one-shot "nearly over" warning is due.
///
/// `warning_sent` makes the warning fire exactly once.
pub fn should_warn(
    window: &ClassWindow,
    now: &Zoned,
    threshold: Duration,
    warning_sent: bool,
) -> bool {
    if warning_sent {
        return false;
    }
    let remaining = minutes_until(&window.ends_at.to_zoned(now.time_zone().clone()), now);
    let threshold_seconds = i64::try_from(threshold.as_secs()).unwrap_or(1800);
    remaining.saturating_mul(60) <= threshold_seconds
}

/// Render the "nearly over" warning body.
pub fn warning_message(class: &ScheduledClass, minutes_remaining: i64, attempt: u32) -> String {
    let attempts = attempt.saturating_add(1);
    format!(
        "**- ATTENDANCE WARNING**\n> Course: {}\n> Class: {}\n> Only {minutes_remaining} minutes remaining!\n> Attempts so far: {attempts}",
        class.course_code, class.name
    )
}

/// Render the confirmation body.
pub fn confirmed_message(class: &ScheduledClass, at: &str) -> String {
    format!(
        "**- Attendance Confirmed!**\n> Course: {}\n> Class: {}\n> Time: {at}\n> Status: Attendance successfully submitted",
        class.course_code, class.name
    )
}

/// Render the failure body.
pub fn failed_message(class: &ScheduledClass, attempts: u32) -> String {
    format!(
        "**- Attendance FAILED**\n> Course: {}\n> Status: Attendance submission not confirmed by end of class\n> Total attempts: {attempts}",
        class.course_code
    )
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use jiff::civil::date;
    use jiff::tz::TimeZone;

    use super::{
        Action, ClassWindow, ScheduledClass, Submission, confirmed_message, failed_message,
        next_action, should_warn, warning_message,
    };

    fn class() -> ScheduledClass {
        ScheduledClass {
            termcode: "2604".to_owned(),
            course_code: "ELEC3050SEF".to_owned(),
            name: "Lecture ( Full Time )".to_owned(),
            starts_at: date(2026, 9, 21).at(11, 0, 0, 0),
            ends_at: Some(date(2026, 9, 21).at(12, 50, 0, 0)),
            group: "L01".to_owned(),
            venue: "HKMU C0G01".to_owned(),
        }
    }

    fn hkt(stamp: &str) -> jiff::Zoned {
        stamp
            .parse::<jiff::Timestamp>()
            .expect("valid timestamp")
            .to_zoned(TimeZone::get("Asia/Hong_Kong").expect("valid tz"))
    }

    fn window() -> ClassWindow {
        ClassWindow::resolve(
            &class(),
            Duration::from_secs(10800),
            &TimeZone::get("Asia/Hong_Kong").expect("tz"),
        )
        .expect("resolves")
    }

    #[test]
    fn classifies_a_confirmed_submission() {
        // Given the server's success vocabulary as the page tests it.
        // When classified.
        // Then present and active are both confirmed, matching the checkmark.
        assert!(Submission::classify("success|2026-09-21 09:03|Present|btn1").is_confirmed());
        assert!(Submission::classify("present").is_confirmed());
        assert!(Submission::classify("active").is_confirmed());
    }

    #[test]
    fn does_not_treat_the_success_envelope_as_confirmation() {
        // Given an envelope whose status is not an attendance-recording status.
        // When classified.
        let outcome = Submission::classify("success|2026-09-21 09:03|Absent|btn1");

        // Then it is NOT confirmed: reporting attendance the page shows as
        // absent would silently lose a class.
        assert!(!outcome.is_confirmed());
        assert_eq!(outcome, Submission::NotYet);
    }

    #[test]
    fn does_not_read_a_word_containing_a_status_token_as_confirmation() {
        // Given bodies that contain "present" or "active" only as part of a
        // longer word, as a revoked-session page does.
        let inactive = Submission::classify("Your session is inactive, please log in again");
        let represent = Submission::classify("This is not representative of attendance");

        // When classified.
        // Then neither is confirmed. Reporting these as confirmed would stop
        // polling and tell the operator a class was safe when it was not.
        assert!(!inactive.is_confirmed(), "got {inactive:?}");
        assert!(!represent.is_confirmed(), "got {represent:?}");
    }

    #[test]
    fn does_not_read_a_bare_substring_of_a_status_field_as_confirmation() {
        // Given an envelope whose status field merely contains a token.
        // When classified.
        // Then it is not confirmed, because the field must match as a whole.
        assert!(!Submission::classify("success|2026-09-21 09:03|Inactive|btn1").is_confirmed());
        assert!(!Submission::classify("success|2026-09-21 09:03|NotPresent|btn1").is_confirmed());
        assert!(!Submission::classify("success|2026-09-21 09:03||btn1").is_confirmed());
    }

    #[test]
    fn classifies_a_closed_submission_window() {
        // Given the exact body the live server returned for a finished activity.
        let outcome = Submission::classify("submission closed-11504");

        // Then it is closed, not success and not unknown.
        assert_eq!(outcome, Submission::Closed);
        assert!(!outcome.is_confirmed());
    }

    #[test]
    fn closed_takes_precedence_over_a_status_token() {
        // Given a closed response that still mentions a status token.
        // When classified.
        // Then closed wins, so a late poll cannot read as confirmation.
        assert_eq!(
            Submission::classify("submission closed: present"),
            Submission::Closed
        );
    }

    #[test]
    fn preserves_an_unrecognised_response_for_diagnostics() {
        // Given an unexpected body.
        // When classified.
        // Then the body is retained so it can be reported.
        assert_eq!(
            Submission::classify("something new"),
            Submission::Unknown("something new".to_owned())
        );
    }

    #[test]
    fn waits_until_the_class_starts() {
        // Given a class starting at 11:00 and a clock at 10:00.
        let now = hkt("2026-09-21T10:00:00+08:00").timestamp();

        // When the next action is decided.
        // Then the poller waits exactly one hour.
        assert_eq!(
            next_action(&window(), now),
            Action::Wait(Duration::from_secs(3600))
        );
    }

    #[test]
    fn polls_once_the_class_is_running() {
        // Given a class running from 11:00 and a clock at 11:30.
        let now = hkt("2026-09-21T11:30:00+08:00").timestamp();

        // When the next action is decided.
        // Then it polls immediately.
        assert_eq!(next_action(&window(), now), Action::Poll);
    }

    #[test]
    fn finishes_once_the_class_is_over() {
        // Given a class ending at 12:50 and a clock at 13:00.
        let now = hkt("2026-09-21T13:00:00+08:00").timestamp();

        // When the next action is decided.
        // Then polling stops.
        assert_eq!(next_action(&window(), now), Action::Finished);
    }

    #[test]
    fn reports_minutes_elapsed_since_start_and_end() {
        // Given a class from 11:00 to 12:50 and a clock at 13:10.
        let now = hkt("2026-09-21T13:10:00+08:00").timestamp();

        // When elapsed minutes are queried.
        // Then both are clamped and correct.
        assert_eq!(window().minutes_since_start(now), 130);
        assert_eq!(window().minutes_since_end(now), 20);
    }

    #[test]
    fn warning_is_not_due_while_the_class_is_young() {
        // Given a class ending at 12:50 and a clock at 11:30.
        let now = hkt("2026-09-21T11:30:00+08:00");

        // When the warning is evaluated.
        // Then it is not due, because 80 minutes remain.
        assert!(!should_warn(
            &window(),
            &now,
            Duration::from_secs(1800),
            false
        ));
    }

    #[test]
    fn warning_is_due_inside_the_threshold() {
        // Given a class ending at 12:50 and a clock at 12:30.
        let now = hkt("2026-09-21T12:30:00+08:00");

        // When the warning is evaluated.
        // Then it is due, because exactly 20 minutes remain.
        assert!(should_warn(
            &window(),
            &now,
            Duration::from_secs(1800),
            false
        ));
    }

    #[test]
    fn warning_fires_only_once() {
        // Given a clock inside the threshold and a warning already sent.
        let now = hkt("2026-09-21T12:30:00+08:00");

        // When the warning is evaluated.
        // Then it is suppressed, so the operator is not paged repeatedly.
        assert!(!should_warn(
            &window(),
            &now,
            Duration::from_secs(1800),
            true
        ));
    }

    #[test]
    fn renders_the_warning_body() {
        // Given a class and the third attempt about to be made.
        let warning = warning_message(&class(), 20, 2);

        // Then it names the class, the remaining time and the attempt count.
        assert!(warning.starts_with("**- ATTENDANCE WARNING**"));
        assert!(warning.contains("> Course: ELEC3050SEF"));
        assert!(warning.contains("> Only 20 minutes remaining!"));
        assert!(warning.contains("> Attempts so far: 3"));
    }

    #[test]
    fn renders_the_confirmed_and_failed_bodies() {
        // Given a class and a completion time.
        let confirmed = confirmed_message(&class(), "2026-09-21 11:03:00 HKT");
        let failed = failed_message(&class(), 7);

        // Then both match the documented headers exactly.
        assert!(confirmed.starts_with("**- Attendance Confirmed!**"));
        assert!(confirmed.contains("> Course: ELEC3050SEF"));
        assert!(confirmed.contains("> Status: Attendance successfully submitted"));
        assert!(failed.starts_with("**- Attendance FAILED**"));
        assert!(failed.contains("> Total attempts: 7"));
    }
}
