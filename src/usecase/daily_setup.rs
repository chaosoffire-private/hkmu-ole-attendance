//! The daily setup flow: fetch the timetable, report it, then schedule polling.

use std::time::Duration;

use tracing::{info, instrument, warn};

use crate::domain::schedule::{DaySchedule, Timetable, format_classes_message, select_day};
use crate::port::session::SessionInvalidator;
use crate::port::{Clock, Notice, Notifier, PortError, ScheduleGateway};

/// How hard to try retrieving a timetable before deferring to the next run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Attempts made to retrieve a usable timetable.
    pub attempts: u32,
    /// Delay between those attempts.
    pub delay: Duration,
}

/// What a daily setup produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetupOutcome {
    /// A timetable was retrieved and reported.
    Ready(DaySchedule),
    /// The service answered, but without a usable timetable.
    Unusable {
        /// The service's own description of the problem.
        reason: String,
    },
}

/// Retrieve today's timetable and report it.
///
/// The day is taken from `clock`, so the flow is testable at any imagined time.
#[instrument(skip(clock, gateway, notifier, invalidator, retry))]
pub async fn daily_setup<C, G, N, I>(
    clock: &C,
    gateway: &G,
    notifier: &N,
    invalidator: &I,
    zone: &jiff::tz::TimeZone,
    retry: RetryPolicy,
) -> Result<SetupOutcome, PortError>
where
    C: Clock,
    G: ScheduleGateway,
    N: Notifier,
    I: SessionInvalidator,
{
    let payload = match fetch_with_retries(gateway, invalidator, retry).await {
        Ok(payload) => payload,
        Err(error) => {
            let message = format!(
                "**Daily Setup Error**\nDate: {}\nError: {error}",
                clock.now(zone).strftime("%Y-%m-%d %H:%M:%S %Z")
            );
            notifier.notify(Notice::Error, &message).await;
            return Err(error);
        }
    };

    let today = clock.now(zone).date();
    let schedule = select_day(&payload, today);
    let stamp = clock.now(zone).strftime("%Y-%m-%d %H:%M:%S %Z").to_string();
    notifier
        .notify(Notice::Info, &format_classes_message(&schedule, &stamp))
        .await;

    if !schedule.is_success() {
        let reason = payload
            .error_summary()
            .unwrap_or_else(|| "unsuccessful result".to_owned());
        warn!(%reason, "class retrieval failed; skipping scheduling");
        return Ok(SetupOutcome::Unusable { reason });
    }

    Ok(SetupOutcome::Ready(schedule))
}

/// Fetch the timetable, retrying until a successful payload arrives.
async fn fetch_with_retries<G, I>(
    gateway: &G,
    invalidator: &I,
    retry: RetryPolicy,
) -> Result<Timetable, PortError>
where
    G: ScheduleGateway,
    I: SessionInvalidator,
{
    let mut last_error: Option<PortError> = None;

    for attempt in 1..=retry.attempts {
        info!(
            attempt,
            attempts = retry.attempts,
            "retrieving today's classes"
        );
        match gateway.today_classes().await {
            Ok(payload) if payload.is_success() => return Ok(payload),
            Ok(payload) => {
                let reason = payload
                    .error_summary()
                    .unwrap_or_else(|| "unsuccessful result".to_owned());
                warn!(attempt, %reason, "class retrieval returned an error payload");
                // An unsuccessful payload is this API's way of saying the
                // session is not usable — the same test `validate` applies — so
                // drop it and let the next attempt log in afresh.
                invalidator.invalidate().await;
                last_error = Some(PortError::Remote(reason));
            }
            Err(error) => {
                warn!(attempt, %error, "class retrieval failed");
                // Only a rejected session justifies a fresh login; a transport
                // blip must not throw away a session that still works.
                if error.is_session_failure() {
                    invalidator.invalidate().await;
                }
                last_error = Some(error);
            }
        }
        if attempt < retry.attempts {
            tokio::time::sleep(retry.delay).await;
        }
    }

    Err(last_error.unwrap_or_else(|| PortError::Remote("no attempt produced a payload".to_owned())))
}

#[cfg(test)]
mod tests {
    use super::{RetryPolicy, SetupOutcome, daily_setup};
    use crate::port::Notice;
    use crate::port::error::PortError;
    use crate::usecase::test_doubles::{
        FakeClock, FakeSessionInvalidator, RecordingNotifier, ScriptedScheduleGateway, hkt,
        rejected_timetable, successful_timetable,
    };

    fn zone() -> jiff::tz::TimeZone {
        jiff::tz::TimeZone::get("Asia/Hong_Kong").expect("valid tz")
    }

    /// Three attempts, matching the default the binary configures.
    fn policy() -> RetryPolicy {
        RetryPolicy {
            attempts: 3,
            delay: std::time::Duration::from_secs(30),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_successful_payload_flows_straight_through() {
        // Given a service that answers successfully on the first attempt.
        let gateway = ScriptedScheduleGateway::new(vec![Ok(successful_timetable())]);
        let notifier = RecordingNotifier::default();
        let invalidator = FakeSessionInvalidator::default();

        // When the daily setup runs.
        let outcome = daily_setup(
            &FakeClock::at(hkt("2026-09-21T03:00:00+08:00")),
            &gateway,
            &notifier,
            &invalidator,
            &zone(),
            policy(),
        )
        .await
        .expect("setup succeeds");

        // Then it reports the retrieved (empty) timetable and never retried,
        // and it did not discard a session that was working.
        assert!(matches!(outcome, SetupOutcome::Ready(_)));
        assert_eq!(gateway.call_count(), 1);
        assert_eq!(invalidator.invalidations(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn a_rejected_payload_discards_the_session_and_retries() {
        // Given a service that rejects the first attempt, as an expired session
        // does, then succeeds once a fresh login has happened.
        let gateway = ScriptedScheduleGateway::new(vec![
            Ok(rejected_timetable()),
            Ok(successful_timetable()),
        ]);
        let notifier = RecordingNotifier::default();
        let invalidator = FakeSessionInvalidator::default();

        // When the daily setup runs.
        let outcome = daily_setup(
            &FakeClock::at(hkt("2026-09-21T03:00:00+08:00")),
            &gateway,
            &notifier,
            &invalidator,
            &zone(),
            policy(),
        )
        .await
        .expect("setup succeeds on the retry");

        // Then the stale session was discarded so the retry could log in afresh.
        assert!(matches!(outcome, SetupOutcome::Ready(_)));
        assert_eq!(gateway.call_count(), 2);
        assert_eq!(
            invalidator.invalidations(),
            1,
            "an unusable payload must discard the session"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_transport_error_does_not_discard_a_working_session() {
        // Given a service that fails at the transport level on the first
        // attempt — a timeout or reset — then answers correctly.
        let gateway = ScriptedScheduleGateway::new(vec![
            Err(PortError::Transport("connection reset".to_owned())),
            Ok(successful_timetable()),
        ]);
        let notifier = RecordingNotifier::default();
        let invalidator = FakeSessionInvalidator::default();

        // When the daily setup runs.
        let outcome = daily_setup(
            &FakeClock::at(hkt("2026-09-21T03:00:00+08:00")),
            &gateway,
            &notifier,
            &invalidator,
            &zone(),
            policy(),
        )
        .await
        .expect("setup succeeds on the retry");

        // Then the session survived, because a network blip is not a dead
        // session and forcing a fresh six-hop login over it would be waste.
        assert!(matches!(outcome, SetupOutcome::Ready(_)));
        assert_eq!(gateway.call_count(), 2);
        assert_eq!(
            invalidator.invalidations(),
            0,
            "a transport failure must not discard a healthy session"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn exhausting_every_attempt_reports_failure_and_notifies() {
        // Given a service that rejects every attempt.
        let gateway = ScriptedScheduleGateway::new(vec![
            Ok(rejected_timetable()),
            Ok(rejected_timetable()),
            Ok(rejected_timetable()),
        ]);
        let notifier = RecordingNotifier::default();
        let invalidator = FakeSessionInvalidator::default();

        // When the daily setup runs.
        let outcome = daily_setup(
            &FakeClock::at(hkt("2026-09-21T03:00:00+08:00")),
            &gateway,
            &notifier,
            &invalidator,
            &zone(),
            policy(),
        )
        .await;

        // Then it gave up after the configured number of attempts, reported the
        // failure to the operator, and never claimed success.
        assert!(outcome.is_err(), "no attempt produced a usable timetable");
        assert_eq!(gateway.call_count(), policy().attempts);
        assert!(notifier.contains("**Daily Setup Error**"));
        assert!(
            !notifier
                .bodies_at(Notice::Info)
                .iter()
                .any(|b| b.contains("Retrieved Classes")),
            "a failed retrieval must not report a class list"
        );
    }
}
