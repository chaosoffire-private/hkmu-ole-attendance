//! Composition root: builds the adapters and drives the use cases.
//!
//! This is the only place that knows both a port and its implementation. The
//! dependency arrow points inward — the use cases never see an adapter.

use std::sync::Arc;

use tokio::task::JoinSet;
use tokio::time::sleep;
use tracing::{error, info, instrument, warn};

use crate::adapter::{
    CacheSessionInvalidator, DiscordNotifier, OleAttendanceGateway, OleScheduleGateway, SystemClock,
};
use crate::config::Config;
use crate::domain::schedule::ScheduledClass;
use crate::domain::time::duration_until_next;
use crate::infra::session_cache::SessionCache;
use crate::port::{Clock, Notice, Notifier, Result as PortResult};
use crate::usecase::{AttendanceOutcome, PollParts, SetupOutcome, daily_setup, mark_attendance};

/// Orchestrates the daily setup and the per-class attendance tasks.
pub struct App {
    config: Arc<Config>,
    notifier: Arc<DiscordNotifier>,
    cache: SessionCache,
    coordinates: Option<(f64, f64)>,
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
    /// Build the application, wiring each port to its adapter.
    pub fn new(config: Config, coordinates: Option<(f64, f64)>) -> Self {
        let notifier = DiscordNotifier::new(
            config
                .discord_webhook
                .as_ref()
                .map(|secret| secret.expose().to_owned()),
        );
        let cache = SessionCache::new(config.clone());
        Self {
            config: Arc::new(config),
            notifier: Arc::new(notifier),
            cache,
            coordinates,
        }
    }

    /// Run one full setup cycle: fetch, report, then schedule each class.
    ///
    /// # Errors
    /// Returns the first fatal error encountered before scheduling begins.
    #[instrument(skip(self), fields(mode = self.config.credentials.label()))]
    pub async fn daily_setup(&self) -> PortResult<SetupOutcome> {
        info!("starting daily attendance setup");
        let clock = SystemClock;
        let gateway =
            OleScheduleGateway::new(self.cache.clone(), self.config.oleconnect_api_url.clone());
        let session = CacheSessionInvalidator::new(self.cache.clone());

        let outcome = daily_setup(
            &clock,
            &gateway,
            self.notifier.as_ref(),
            &session,
            &self.config.timezone,
        )
        .await?;

        if let SetupOutcome::Ready(schedule) = &outcome {
            info!(classes = schedule.classes.len(), "scheduling attendance");
            self.run_schedule(schedule.classes.clone()).await;
        }
        Ok(outcome)
    }

    /// Spawn one polling task per class and wait for every task to finish.
    async fn run_schedule(&self, classes: Vec<ScheduledClass>) {
        let mut set = JoinSet::new();
        for class in classes {
            let config = Arc::clone(&self.config);
            let notifier = Arc::clone(&self.notifier);
            let cache = self.cache.clone();
            let coordinates = self.coordinates;
            set.spawn(async move {
                let clock = SystemClock;
                let gateway = OleAttendanceGateway::new(
                    cache.clone(),
                    config.oleconnect_api_url.clone(),
                    config.ole_url.clone(),
                );
                let session = CacheSessionInvalidator::new(cache);

                mark_attendance(
                    PollParts {
                        clock: &clock,
                        gateway: &gateway,
                        notifier: notifier.as_ref(),
                        session: &session,
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

    /// Run forever, executing a setup cycle at the configured time.
    pub async fn run(self: Arc<Self>) -> PortResult<()> {
        if !self.notifier.is_enabled() {
            warn!("DISCORD_WEBHOOK is unset; notifications will be logged instead");
        }
        let clock = SystemClock;

        if let Err(error) = Arc::clone(&self).daily_setup().await {
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

            if let Err(error) = Arc::clone(&self).daily_setup().await {
                error!(%error, "daily setup failed");
                self.notifier
                    .notify(
                        Notice::Error,
                        &format!("**Daily Setup Error**\nError: {error}"),
                    )
                    .await;
            }
        }
    }
}
