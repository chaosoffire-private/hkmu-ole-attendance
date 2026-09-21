//! The daily setup flow: fetch the timetable, report it, then schedule polling.

use std::time::Duration;

use tracing::{info, instrument, warn};

use crate::domain::schedule::{DaySchedule, format_classes_message, select_day};
use crate::port::session::SessionProvider;
use crate::port::{Clock, Notice, Notifier, PortError, ScheduleGateway};

/// Attempts made to retrieve a usable timetable.
pub const FETCH_ATTEMPTS: u32 = 3;
/// Delay between retrieval attempts.
pub const FETCH_RETRY_DELAY: Duration = Duration::from_secs(30);

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
#[instrument(skip(clock, gateway, notifier, session))]
pub async fn daily_setup<C, G, N, S>(
    clock: &C,
    gateway: &G,
    notifier: &N,
    session: &S,
    zone: &jiff::tz::TimeZone,
) -> Result<SetupOutcome, PortError>
where
    C: Clock,
    G: ScheduleGateway,
    N: Notifier,
    S: SessionProvider,
{
    let payload = match fetch_with_retries(gateway, session).await {
        Ok(payload) => payload,
        Err(error) => {
            let message = format!(
                "**Daily Setup Error**\nDate: {}\nError: {error}",
                clock.now(zone).strftime("%Y-%m-%d %H:%M:%S %Z")
            );
            let _ = notifier.notify(Notice::Error, &message).await;
            return Err(error);
        }
    };

    let today = clock.now(zone).date();
    let schedule = select_day(&payload, today);
    let stamp = clock.now(zone).strftime("%Y-%m-%d %H:%M:%S %Z").to_string();
    let _ = notifier
        .notify(Notice::Info, &format_classes_message(&schedule, &stamp))
        .await;

    if !schedule.result.is_success() {
        let reason = payload
            .error_summary()
            .unwrap_or_else(|| "unsuccessful result".to_owned());
        warn!(%reason, "class retrieval failed; skipping scheduling");
        return Ok(SetupOutcome::Unusable { reason });
    }

    Ok(SetupOutcome::Ready(schedule))
}

/// Fetch the timetable, retrying until a successful payload arrives.
async fn fetch_with_retries<G, S>(
    gateway: &G,
    session: &S,
) -> Result<crate::domain::schedule::TodayClassResponse, PortError>
where
    G: ScheduleGateway,
    S: SessionProvider,
{
    let mut last_error: Option<PortError> = None;

    for attempt in 1..=FETCH_ATTEMPTS {
        info!(
            attempt,
            attempts = FETCH_ATTEMPTS,
            "retrieving today's classes"
        );
        match gateway.today_classes().await {
            Ok(payload) if payload.is_success() => return Ok(payload),
            Ok(payload) => {
                let reason = payload
                    .error_summary()
                    .unwrap_or_else(|| "unsuccessful result".to_owned());
                warn!(attempt, %reason, "class retrieval returned an error payload");
                // A rejected session is recoverable: drop it so the next
                // attempt logs in again rather than replaying a dead cookie.
                session.invalidate().await;
                last_error = Some(PortError::Remote(reason));
            }
            Err(error) => {
                warn!(attempt, %error, "class retrieval failed");
                last_error = Some(error);
            }
        }
        if attempt < FETCH_ATTEMPTS {
            tokio::time::sleep(FETCH_RETRY_DELAY).await;
        }
    }

    Err(last_error.unwrap_or_else(|| PortError::Remote("no attempt produced a payload".to_owned())))
}
