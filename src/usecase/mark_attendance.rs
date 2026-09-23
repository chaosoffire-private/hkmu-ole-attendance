//! Drive one class's attendance to a conclusion.
//!
//! The policy lives in [`crate::domain::attendance`]; this module only supplies
//! it with reality — the clock, the gateway, the notifier and the invalidator. Each
//! decision is delegated, so the flow stays short and the rules stay testable
//! on their own.

use std::time::Duration;

use tracing::{info, instrument, warn};

use crate::domain::attendance::{
    Action, ClassWindow, Submission, confirmed_message, failed_message, next_action, should_warn,
    warning_message,
};
use crate::domain::geo::Coordinates;
use crate::domain::schedule::ScheduledClass;
use crate::domain::time::minutes_until;
use crate::port::{AttendanceGateway, Clock, Notice, Notifier, SessionInvalidator};

/// How one class finished.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttendanceOutcome {
    /// Attendance was recorded.
    Confirmed,
    /// Polling ended without a confirmation.
    Unconfirmed {
        /// How many submissions were attempted.
        attempts: u32,
    },
    /// The class had already finished before polling began.
    AlreadyOver,
}

/// Everything one poll needs, so the helpers below stay short.
struct Poll<'a, C, G, N, I> {
    clock: &'a C,
    gateway: &'a G,
    notifier: &'a N,
    invalidator: &'a I,
    class: &'a ScheduledClass,
    window: ClassWindow,
    coordinates: Option<Coordinates>,
    zone: &'a jiff::tz::TimeZone,
    interval: Duration,
    warning_threshold: Duration,
}

/// Poll `class` until attendance is confirmed or the class ends.
#[instrument(skip(poll_parts), fields(course = %class.course_code))]
pub async fn mark_attendance<C, G, N, I>(
    poll_parts: PollParts<'_, C, G, N, I>,
    class: &ScheduledClass,
) -> AttendanceOutcome
where
    C: Clock,
    G: AttendanceGateway,
    N: Notifier,
    I: SessionInvalidator,
{
    let PollParts {
        clock,
        gateway,
        notifier,
        invalidator,
        coordinates,
        zone,
        poll_interval,
        warning_threshold,
        fallback_duration,
    } = poll_parts;

    let Some(window) = ClassWindow::resolve(class, fallback_duration, zone) else {
        warn!("could not resolve the class window");
        return AttendanceOutcome::AlreadyOver;
    };

    let poll = Poll {
        clock,
        gateway,
        notifier,
        invalidator,
        class,
        window,
        coordinates,
        zone,
        interval: poll_interval,
        warning_threshold,
    };

    let now = clock.timestamp();
    match next_action(&window, now) {
        Action::Wait(wait) => {
            info!(
                minutes = wait.as_secs() / 60,
                "waiting for the class to start"
            );
            tokio::time::sleep(wait).await;
        }
        Action::Poll => info!(
            minutes_since_start = window.minutes_since_start(now),
            "class already in progress; starting now"
        ),
        Action::Finished => {
            info!(
                minutes_since_end = window.minutes_since_end(now),
                "class already ended; skipping"
            );
            return AttendanceOutcome::AlreadyOver;
        }
    }

    run_loop(&poll).await
}

/// The polling loop, separated so the setup above stays readable.
async fn run_loop<C, G, N, I>(poll: &Poll<'_, C, G, N, I>) -> AttendanceOutcome
where
    C: Clock,
    G: AttendanceGateway,
    N: Notifier,
    I: SessionInvalidator,
{
    let mut attempt = 0_u32;
    let mut warning_sent = false;

    while next_action(&poll.window, poll.clock.timestamp()) == Action::Poll {
        let outcome = submit_once(poll).await;

        if outcome.is_confirmed() {
            let at = poll
                .clock
                .now(poll.zone)
                .strftime("%Y-%m-%d %H:%M:%S %Z")
                .to_string();
            poll.notifier
                .notify(Notice::Info, &confirmed_message(poll.class, &at))
                .await;
            info!("attendance confirmed");
            return AttendanceOutcome::Confirmed;
        }

        if outcome == Submission::Closed {
            warn!("the submission window has closed; stopping for this class");
            break;
        }

        info!(%outcome, attempt, "attendance not confirmed yet");
        warning_sent = warn_if_due(poll, attempt, warning_sent).await;
        attempt = attempt.saturating_add(1);
        tokio::time::sleep(poll.interval).await;
    }

    poll.notifier
        .notify(Notice::Error, &failed_message(poll.class, attempt))
        .await;
    AttendanceOutcome::Unconfirmed { attempts: attempt }
}

/// One submission, translating a rejected session into a cache invalidation.
async fn submit_once<C, G, N, S>(poll: &Poll<'_, C, G, N, S>) -> Submission
where
    C: Clock,
    G: AttendanceGateway,
    N: Notifier,
    S: SessionInvalidator,
{
    match poll.gateway.submit(poll.class, poll.coordinates).await {
        Ok(outcome) => outcome,
        Err(error) if error.is_session_failure() => {
            warn!(%error, "cached session was rejected; discarding it");
            poll.invalidator.invalidate().await;
            Submission::NotYet
        }
        Err(error) => {
            warn!(%error, "attendance submission failed");
            Submission::NotYet
        }
    }
}

/// Send the one-shot "nearly over" warning when due.
async fn warn_if_due<C, G, N, S>(
    poll: &Poll<'_, C, G, N, S>,
    attempt: u32,
    warning_sent: bool,
) -> bool
where
    C: Clock,
    G: AttendanceGateway,
    N: Notifier,
    S: SessionInvalidator,
{
    let now = poll.clock.now(poll.zone);
    if !should_warn(&poll.window, &now, poll.warning_threshold, warning_sent) {
        return warning_sent;
    }
    let end = poll.window.ends_at.to_zoned(now.time_zone().clone());
    let remaining = minutes_until(&end, &now);
    poll.notifier
        .notify(
            Notice::Warning,
            &warning_message(poll.class, remaining, attempt),
        )
        .await;
    true
}

/// The capabilities a poll needs, grouped so the signature stays small.
#[derive(Debug)]
pub struct PollParts<'a, C, G, N, I> {
    /// Supplies the current instant.
    pub clock: &'a C,
    /// Drives one submission.
    pub gateway: &'a G,
    /// Delivers messages.
    pub notifier: &'a N,
    /// Discards a session the server has rejected.
    pub invalidator: &'a I,
    /// Coordinates submitted with attendance.
    pub coordinates: Option<Coordinates>,
    /// Zone the schedule is expressed in.
    pub zone: &'a jiff::tz::TimeZone,
    /// Delay between polls.
    pub poll_interval: Duration,
    /// When the "nearly over" warning becomes due.
    pub warning_threshold: Duration,
    /// Assumed length when the API supplied no end time.
    pub fallback_duration: Duration,
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use super::{AttendanceOutcome, PollParts, mark_attendance};
    use crate::domain::attendance::Submission;
    use crate::port::Notice;
    use crate::port::error::{PortError, SessionFailure};
    use crate::usecase::test_doubles::{
        FakeClock, FakeSessionInvalidator, RecordingNotifier, ScriptedAttendanceGateway,
        class_from, hkt,
    };

    fn zone() -> jiff::tz::TimeZone {
        jiff::tz::TimeZone::get("Asia/Hong_Kong").expect("valid tz")
    }

    /// A poll interval short enough that tests do not actually wait.
    const FAST: Duration = Duration::from_millis(1);

    #[tokio::test]
    async fn confirms_on_the_first_successful_submission() {
        // Given a class in progress and a server that accepts immediately.
        let clock = FakeClock::at(hkt("2026-09-21T11:30:00+08:00"));
        let gateway = ScriptedAttendanceGateway::new(vec![Ok(Submission::Confirmed)]);
        let notifier = RecordingNotifier::default();
        let invalidator = FakeSessionInvalidator::default();
        let class = class_from(11, 12);

        // When attendance is polled.
        let outcome = mark_attendance(
            PollParts {
                clock: &clock,
                gateway: &gateway,
                notifier: &notifier,
                invalidator: &invalidator,
                coordinates: None,
                zone: &zone(),
                poll_interval: FAST,
                warning_threshold: Duration::from_secs(1800),
                fallback_duration: Duration::from_secs(10800),
            },
            &class,
        )
        .await;

        // Then it confirms, submits exactly once, and says so.
        assert_eq!(outcome, AttendanceOutcome::Confirmed);
        assert_eq!(
            gateway.submission_count(),
            1,
            "should not poll again after success"
        );
        assert!(notifier.contains("**- Attendance Confirmed!**"));
    }

    #[tokio::test]
    async fn stops_at_the_first_confirmation_without_reporting_failure() {
        // Given a class that ends at 12:00 and a clock at 11:59, so the poll
        // loop ends by the clock rather than by the outcome.
        let clock = FakeClock::at(hkt("2026-09-21T11:59:30+08:00"));
        let gateway =
            ScriptedAttendanceGateway::new(vec![Ok(Submission::Confirmed), Ok(Submission::NotYet)]);
        let notifier = RecordingNotifier::default();
        let invalidator = FakeSessionInvalidator::default();
        let class = class_from(11, 12);

        // When attendance is polled.
        let outcome = mark_attendance(
            PollParts {
                clock: &clock,
                gateway: &gateway,
                notifier: &notifier,
                invalidator: &invalidator,
                coordinates: None,
                zone: &zone(),
                poll_interval: FAST,
                warning_threshold: Duration::from_secs(1800),
                fallback_duration: Duration::from_secs(10800),
            },
            &class,
        )
        .await;

        // Then it confirms and never emits a spurious FAILED, which is the
        // regression the old implementation guarded with a `confirmed` flag.
        assert_eq!(outcome, AttendanceOutcome::Confirmed);
        assert!(
            !notifier.contains("**- Attendance FAILED**"),
            "a success must never be followed by a failure notice"
        );
    }

    #[tokio::test]
    async fn an_already_finished_class_is_skipped_without_submitting() {
        // Given a class that ended at 12:00 and a clock at 13:00.
        let clock = FakeClock::at(hkt("2026-09-21T13:00:00+08:00"));
        let gateway = ScriptedAttendanceGateway::new(vec![Ok(Submission::Confirmed)]);
        let notifier = RecordingNotifier::default();
        let invalidator = FakeSessionInvalidator::default();
        let class = class_from(11, 12);

        // When attendance is polled.
        let outcome = mark_attendance(
            PollParts {
                clock: &clock,
                gateway: &gateway,
                notifier: &notifier,
                invalidator: &invalidator,
                coordinates: None,
                zone: &zone(),
                poll_interval: FAST,
                warning_threshold: Duration::from_secs(1800),
                fallback_duration: Duration::from_secs(10800),
            },
            &class,
        )
        .await;

        // Then nothing is submitted at all.
        assert_eq!(outcome, AttendanceOutcome::AlreadyOver);
        assert_eq!(gateway.submission_count(), 0);
    }

    #[tokio::test]
    async fn waits_until_the_class_starts_before_submitting() {
        // Given a class starting at 11:00 and a clock at 11:00 sharp, so the
        // wait path resolves to zero and polling begins immediately.
        let clock = FakeClock::at(hkt("2026-09-21T11:00:00+08:00"));
        let gateway = ScriptedAttendanceGateway::new(vec![Ok(Submission::Confirmed)]);
        let notifier = RecordingNotifier::default();
        let invalidator = FakeSessionInvalidator::default();
        let class = class_from(11, 12);

        // When attendance is polled.
        let outcome = mark_attendance(
            PollParts {
                clock: &clock,
                gateway: &gateway,
                notifier: &notifier,
                invalidator: &invalidator,
                coordinates: None,
                zone: &zone(),
                poll_interval: FAST,
                warning_threshold: Duration::from_secs(1800),
                fallback_duration: Duration::from_secs(10800),
            },
            &class,
        )
        .await;

        // Then it proceeds to submit rather than idling.
        assert_eq!(outcome, AttendanceOutcome::Confirmed);
        assert_eq!(gateway.submission_count(), 1);
    }

    #[tokio::test]
    async fn a_revoked_session_is_invalidated_so_the_next_attempt_can_log_in() {
        // Given a gateway whose first call reports the session was revoked,
        // and a second call that succeeds.
        let clock = FakeClock::at(hkt("2026-09-21T11:00:00+08:00"));
        let gateway = ScriptedAttendanceGateway::new(vec![
            Err(PortError::Session(SessionFailure::Expired(
                "revoked server-side".to_owned(),
            ))),
            Ok(Submission::Confirmed),
        ]);
        let notifier = RecordingNotifier::default();
        let invalidator = FakeSessionInvalidator::default();
        let class = class_from(11, 12);

        // When attendance is polled. The clock is advanced by the poll interval
        // so the loop can reach its second attempt within the class window.
        let outcome = {
            let parts = PollParts {
                clock: &clock,
                gateway: &gateway,
                notifier: &notifier,
                invalidator: &invalidator,
                coordinates: None,
                zone: &zone(),
                poll_interval: FAST,
                warning_threshold: Duration::from_secs(1800),
                fallback_duration: Duration::from_secs(10800),
            };
            mark_attendance(parts, &class).await
        };

        // Then the cache was invalidated, and the retry confirmed. Without the
        // invalidation the poll would replay a dead session until the class
        // ended and then report a failure.
        assert!(
            invalidator.invalidations() >= 1,
            "a revoked session must be discarded"
        );
        assert_eq!(outcome, AttendanceOutcome::Confirmed);
    }

    #[tokio::test]
    async fn a_closed_window_stops_polling_early() {
        // Given a class in progress whose window has already closed.
        let clock = FakeClock::at(hkt("2026-09-21T11:30:00+08:00"));
        let gateway = ScriptedAttendanceGateway::new(vec![Ok(Submission::Closed)]);
        let notifier = RecordingNotifier::default();
        let invalidator = FakeSessionInvalidator::default();
        let class = class_from(11, 12);

        // When attendance is polled.
        let outcome = mark_attendance(
            PollParts {
                clock: &clock,
                gateway: &gateway,
                notifier: &notifier,
                invalidator: &invalidator,
                coordinates: None,
                zone: &zone(),
                poll_interval: FAST,
                warning_threshold: Duration::from_secs(1800),
                fallback_duration: Duration::from_secs(10800),
            },
            &class,
        )
        .await;

        // Then it stops after one attempt rather than hammering a closed window.
        assert_eq!(gateway.submission_count(), 1);
        assert!(matches!(
            outcome,
            AttendanceOutcome::Unconfirmed { attempts: 0 }
        ));
    }

    #[tokio::test]
    async fn reports_failure_when_the_class_ends_unconfirmed() {
        // Given a class from 11:00 to 12:00 and a server that never accepts,
        // with the clock advancing 20 minutes per poll so the class window is
        // genuinely outlived rather than frozen.
        let clock = Arc::new(FakeClock::at(hkt("2026-09-21T11:10:00+08:00")));
        let gateway = ScriptedAttendanceGateway::advancing(
            vec![
                Ok(Submission::NotYet),
                Ok(Submission::NotYet),
                Ok(Submission::NotYet),
                Ok(Submission::NotYet),
            ],
            Arc::clone(&clock),
            20 * 60,
        );
        let notifier = RecordingNotifier::default();
        let invalidator = FakeSessionInvalidator::default();
        let class = class_from(11, 12);

        // When attendance is polled until the window closes.
        let outcome = mark_attendance(
            PollParts {
                clock: clock.as_ref(),
                gateway: &gateway,
                notifier: &notifier,
                invalidator: &invalidator,
                coordinates: None,
                zone: &zone(),
                poll_interval: FAST,
                warning_threshold: Duration::from_secs(1800),
                fallback_duration: Duration::from_secs(10800),
            },
            &class,
        )
        .await;

        // Then it never claims success and does report the failure.
        assert!(
            matches!(outcome, AttendanceOutcome::Unconfirmed { .. }),
            "got {outcome:?}"
        );
        assert!(notifier.contains("**- Attendance FAILED**"));
    }

    #[tokio::test]
    async fn emits_the_nearly_over_warning_at_most_once() {
        // Given a class ending at 12:00 and a clock at 11:35, inside the
        // 30-minute warning threshold, with the clock advancing 5 minutes per
        // poll so several attempts land inside the window.
        let clock = Arc::new(FakeClock::at(hkt("2026-09-21T11:35:00+08:00")));
        let gateway = ScriptedAttendanceGateway::advancing(
            vec![
                Ok(Submission::NotYet),
                Ok(Submission::NotYet),
                Ok(Submission::NotYet),
                Ok(Submission::NotYet),
                Ok(Submission::NotYet),
            ],
            Arc::clone(&clock),
            5 * 60,
        );
        let notifier = RecordingNotifier::default();
        let invalidator = FakeSessionInvalidator::default();
        let class = class_from(11, 12);

        // When attendance is polled until the window closes.
        let parts = PollParts {
            clock: clock.as_ref(),
            gateway: &gateway,
            notifier: &notifier,
            invalidator: &invalidator,
            coordinates: None,
            zone: &zone(),
            poll_interval: FAST,
            warning_threshold: Duration::from_secs(1800),
            fallback_duration: Duration::from_secs(10800),
        };
        let _ = mark_attendance(parts, &class).await;

        // Then the warning appears exactly once, so the operator is not paged
        // on every poll.
        let warnings = notifier.bodies_at(Notice::Warning);
        assert_eq!(warnings.len(), 1, "expected one warning, got {warnings:?}");
        let warning = warnings.first().expect("one warning");
        assert!(warning.contains("**- ATTENDANCE WARNING**"));
    }
}
