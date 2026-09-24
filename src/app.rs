//! The assembly point: binds every port to its adapter, then drives the use cases.
//!
//! This is the only module that knows both a port and its implementation: the
//! private factory methods below are the sole place an adapter is constructed.
//! Each entry point lives here too — the daily scheduler, a one-shot fetch, and
//! a read-only probe — so the whole wiring can be read in one place.

use std::sync::Arc;

use tokio::task::JoinSet;
use tokio::time::sleep;
use tracing::{error, info, instrument, warn};

use crate::adapter::{
    BroadcastNotifier, CacheSessionInvalidator, DiscordNotifier, LogNotifier, OleAttendanceGateway,
    OleScheduleGateway, SystemClock,
};
use crate::config::Config;
use crate::domain::geo::Coordinates;
use crate::domain::schedule::ScheduledClass;
use crate::domain::time::duration_until_next;
use crate::infra::session_cache::SessionCache;
use crate::port::{
    ActivityState, AttendanceGateway, Clock, Notice, Notifier, PortError, Result as PortResult,
};
use crate::usecase::{
    AttendanceOutcome, PollParts, RetryPolicy, SetupOutcome, daily_setup, mark_attendance,
};

/// Where a one-shot report is delivered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Report {
    /// Written to the log only.
    Log,
    /// Written to the log, and posted to Discord when a webhook is configured.
    All,
}

/// Builds the adapters once and drives every use case over them.
pub struct App {
    config: Arc<Config>,
    cache: SessionCache,
    coordinates: Option<Coordinates>,
    notifier: Arc<BroadcastNotifier>,
}

impl std::fmt::Debug for App {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("App")
            .field("config", &self.config)
            .field("coordinates", &self.coordinates)
            .finish_non_exhaustive()
    }
}

impl App {
    /// Build the application, ready to run any entry point.
    pub fn new(config: Config, coordinates: Option<Coordinates>) -> Self {
        let cache = SessionCache::new(config.clone());
        let notifier = Arc::new(notifiers(&config));
        Self {
            config: Arc::new(config),
            cache,
            coordinates,
            notifier,
        }
    }

    /// Run forever, executing a setup cycle at the configured time.
    ///
    /// Never returns: after each cycle it sleeps until the next scheduled run.
    pub async fn run_scheduler(&self) {
        if self.config.discord_webhook.is_none() {
            warn!("DISCORD_WEBHOOK is unset; notifications will be logged instead");
        }
        let clock = SystemClock;

        if let Err(error) = self.run_setup_cycle().await {
            warn!(%error, "initial setup failed; the daily timer will retry");
        }

        loop {
            let wait =
                duration_until_next(self.config.schedule_time, &clock.now(&self.config.timezone));
            info!(
                seconds = wait.as_secs(),
                tz = %self.config.timezone_name,
                at = %self.config.schedule_time,
                "waiting for the next daily run"
            );
            sleep(wait).await;

            if let Err(error) = self.run_setup_cycle().await {
                // `daily_setup` has already notified and logged the failure, so
                // this only records that the run as a whole did not complete.
                warn!(%error, "daily setup failed; the daily timer will retry");
            }
        }
    }

    /// Fetch today's timetable and report it, then return.
    ///
    /// # Errors
    /// Returns the first fatal error from the fetch.
    pub async fn fetch_once(&self, report: Report) -> PortResult<()> {
        match report {
            Report::All => self.report_timetable(self.notifier.as_ref()).await,
            Report::Log => self.report_timetable(&LogNotifier).await,
        }
    }

    /// Fetch the timetable and report it through `notifier`.
    async fn report_timetable<N: Notifier>(&self, notifier: &N) -> PortResult<()> {
        let clock = SystemClock;
        daily_setup(
            &clock,
            &self.schedule_gateway(),
            notifier,
            &self.invalidator(),
            &self.config.timezone,
            self.retry_policy(),
        )
        .await?;
        Ok(())
    }

    /// Report whether each of today's classes has a reachable attendance activity.
    ///
    /// Read-only: it locates the activity but never records attendance, so it is
    /// safe to run at any time.
    ///
    /// # Errors
    /// Returns an error when the timetable cannot be fetched or is unusable.
    pub async fn probe(&self) -> PortResult<()> {
        let clock = SystemClock;
        let invalidator = self.invalidator();
        let notifier = LogNotifier;

        let outcome = daily_setup(
            &clock,
            &self.schedule_gateway(),
            &notifier,
            &invalidator,
            &self.config.timezone,
            self.retry_policy(),
        )
        .await?;
        let SetupOutcome::Ready(schedule) = outcome else {
            return Err(PortError::Unexpected(
                "no usable timetable to probe".to_owned(),
            ));
        };
        if schedule.classes.is_empty() {
            return Err(PortError::Unexpected(
                "no classes scheduled today to probe".to_owned(),
            ));
        }

        let gateway = self.attendance_gateway();
        for class in &schedule.classes {
            match gateway.probe(class).await {
                Ok(state) => report_probe(&notifier, class, state).await,
                Err(error) => {
                    notifier
                        .notify(
                            Notice::Error,
                            &format!("{} | probe failed: {error}", class.course_code),
                        )
                        .await;
                }
            }
        }
        Ok(())
    }

    /// Run one full setup cycle: fetch, report, then schedule each class.
    #[instrument(skip(self), fields(mode = self.config.credentials.label()))]
    async fn run_setup_cycle(&self) -> PortResult<SetupOutcome> {
        info!("starting daily attendance setup");
        let clock = SystemClock;
        let outcome = daily_setup(
            &clock,
            &self.schedule_gateway(),
            self.notifier.as_ref(),
            &self.invalidator(),
            &self.config.timezone,
            self.retry_policy(),
        )
        .await?;

        if let SetupOutcome::Ready(schedule) = &outcome {
            info!(classes = schedule.classes.len(), "scheduling attendance");
            self.run_attendance_tasks(schedule.classes.clone()).await;
        }
        Ok(outcome)
    }

    /// Spawn one polling task per class and wait for every task to finish.
    async fn run_attendance_tasks(&self, classes: Vec<ScheduledClass>) {
        let mut set = JoinSet::new();
        for class in classes {
            let config = Arc::clone(&self.config);
            let notifier = Arc::clone(&self.notifier);
            let gateway = self.attendance_gateway();
            let invalidator = self.invalidator();
            let coordinates = self.coordinates;
            set.spawn(async move {
                let clock = SystemClock;
                mark_attendance(
                    PollParts {
                        clock: &clock,
                        gateway: &gateway,
                        notifier: notifier.as_ref(),
                        invalidator: &invalidator,
                        coordinates,
                        zone: &config.timezone,
                        poll_interval: config.attendance_poll_interval,
                        warning_threshold: config.warning_threshold,
                        fallback_duration: config.default_class_duration,
                    },
                    &class,
                )
                .await
            });
        }

        while let Some(joined) = set.join_next().await {
            match joined {
                Ok(AttendanceOutcome::Confirmed | AttendanceOutcome::AlreadyOver) => {}
                Ok(AttendanceOutcome::Unconfirmed { attempts }) => {
                    warn!(attempts, "a class finished without a confirmation");
                }
                Err(join_error) if join_error.is_panic() => {
                    error!(?join_error, "attendance task panicked");
                }
                Err(join_error) => error!(?join_error, "attendance task was cancelled"),
            }
        }
    }

    /// The retry policy the configured timetable fetch runs with.
    fn retry_policy(&self) -> RetryPolicy {
        RetryPolicy {
            attempts: self.config.setup_retry_attempts,
            delay: self.config.setup_retry_delay,
        }
    }

    /// A timetable reader bound to the shared session cache.
    fn schedule_gateway(&self) -> OleScheduleGateway {
        OleScheduleGateway::new(self.cache.clone(), self.config.oleconnect_api_url.clone())
    }

    /// An attendance driver bound to the shared session cache.
    fn attendance_gateway(&self) -> OleAttendanceGateway {
        OleAttendanceGateway::new(
            self.cache.clone(),
            self.config.oleconnect_api_url.clone(),
            self.config.ole_url.clone(),
        )
    }

    /// The session invalidator the use cases see.
    fn invalidator(&self) -> CacheSessionInvalidator {
        CacheSessionInvalidator::new(self.cache.clone())
    }
}

/// Assemble the destinations notifications are delivered to.
///
/// Logging is always present; Discord joins it only when a webhook is
/// configured. Whether a transport exists is decided here, at assembly, so the
/// transports themselves stay unconditional. Any further `Notifier` joins the
/// same list without the broadcast needing to know its type.
fn notifiers(config: &Config) -> BroadcastNotifier {
    let mut destinations: Vec<Box<dyn Notifier>> = vec![Box::new(LogNotifier)];
    if let Some(webhook) = &config.discord_webhook {
        destinations.push(Box::new(DiscordNotifier::new(webhook.expose().to_owned())));
    }
    BroadcastNotifier::new(destinations)
}

/// Report one probe result at the severity it deserves.
async fn report_probe<N: Notifier>(notifier: &N, class: &ScheduledClass, state: ActivityState) {
    let (level, message) = match state {
        ActivityState::Open => (
            Notice::Info,
            format!("{} | attendance activity is open", class.course_code),
        ),
        ActivityState::Closed => (
            Notice::Warning,
            format!(
                "{} | attendance activity found but its window is closed",
                class.course_code
            ),
        ),
        ActivityState::Absent => (
            Notice::Warning,
            format!("{} | no attendance activity found", class.course_code),
        ),
    };
    notifier.notify(level, &message).await;
}
