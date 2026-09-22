//! Entry point: parse arguments, load configuration, and run.

use std::sync::Arc;

use anyhow::Context as _;
use clap::Parser;
use hkmu_ole_attendance::adapter::{
    CacheSessionInvalidator, DiscordNotifier, OleAttendanceGateway, OleScheduleGateway, SystemClock,
};
use hkmu_ole_attendance::config::Config;
use hkmu_ole_attendance::infra::SessionCache;
use hkmu_ole_attendance::port::{AttendanceGateway, Notice, Notifier};
use hkmu_ole_attendance::scheduler::App;
use hkmu_ole_attendance::usecase::daily_setup;
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(
    name = "hkmu-ole-attendance",
    version,
    about = "Automatic HKMU OLE class-activity attendance"
)]
struct Cli {
    /// Authenticate, fetch today's classes, log them, and exit.
    #[arg(long)]
    fetch_only: bool,

    /// Send today's class list to the configured Discord webhook and exit.
    #[arg(long)]
    notify_only: bool,

    /// Report whether each class's attendance activity is reachable, without
    /// submitting anything.
    #[arg(long)]
    probe: bool,

    /// Coordinates submitted with attendance in the scheduled run, as
    /// "lat,lng" (default: 0,0).
    #[arg(long, value_name = "LAT,LNG")]
    coordinates: Option<String>,
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,hyper=warn,reqwest=warn")),
        )
        .with_target(false)
        .compact()
        .init();

    let _ = dotenvy::dotenv();

    let cli = Cli::parse();
    let config = Config::from_env().context("loading configuration")?;
    let coordinates = parse_coordinates(cli.coordinates.as_deref())?;

    if cli.probe {
        return run_probe(&config).await;
    }

    if cli.fetch_only || cli.notify_only {
        return run_once(&config, cli.notify_only).await;
    }

    let app = Arc::new(App::new(config, coordinates));
    app.run().await.context("running the scheduler")?;
    Ok(())
}

/// Fetch today's classes and report them through the chosen notifier.
async fn run_once(config: &Config, notify: bool) -> anyhow::Result<()> {
    let clock = SystemClock;
    let cache = SessionCache::new(config.clone());
    let gateway = OleScheduleGateway::new(cache.clone(), config.oleconnect_api_url.clone());
    let session = CacheSessionInvalidator::new(cache);

    // Without --notify-only every notice stays in the log; with it, the same
    // call also reaches Discord. One path, one flag.
    let notifier = DiscordNotifier::new(if notify {
        config
            .discord_webhook
            .as_ref()
            .map(|secret| secret.expose().to_owned())
    } else {
        None
    });

    daily_setup(&clock, &gateway, &notifier, &session, &config.timezone)
        .await
        .context("running daily setup")?;
    Ok(())
}

/// Report whether each of today's classes has a reachable attendance activity.
///
/// Read-only: it locates the activity but never records attendance, so it is
/// safe to run at any time.
async fn run_probe(config: &Config) -> anyhow::Result<()> {
    let clock = SystemClock;
    let cache = SessionCache::new(config.clone());
    let schedule_gateway =
        OleScheduleGateway::new(cache.clone(), config.oleconnect_api_url.clone());
    let session = CacheSessionInvalidator::new(cache.clone());
    let notifier = hkmu_ole_attendance::adapter::LogNotifier;

    // Reuse the daily setup flow so the timetables come from one code path.
    let outcome = daily_setup(
        &clock,
        &schedule_gateway,
        &notifier,
        &session,
        &config.timezone,
    )
    .await
    .context("fetching classes")?;
    let hkmu_ole_attendance::usecase::SetupOutcome::Ready(schedule) = outcome else {
        anyhow::bail!("no usable timetable to probe");
    };
    anyhow::ensure!(
        !schedule.classes.is_empty(),
        "no classes scheduled today to probe"
    );

    let gateway = OleAttendanceGateway::new(
        cache.clone(),
        config.oleconnect_api_url.clone(),
        config.ole_url.clone(),
    );

    for class in &schedule.classes {
        match gateway.probe(class).await {
            Ok(report) if report.found && report.open => {
                notifier
                    .notify(
                        Notice::Info,
                        &format!("{} | attendance activity is open", class.course_code),
                    )
                    .await;
            }
            Ok(report) if report.found => {
                notifier
                    .notify(
                        Notice::Warning,
                        &format!(
                            "{} | attendance activity found but its window is closed",
                            class.course_code
                        ),
                    )
                    .await;
            }
            Ok(_) => {
                notifier
                    .notify(
                        Notice::Warning,
                        &format!("{} | no attendance activity found", class.course_code),
                    )
                    .await;
            }
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

/// Parse a `lat,lng` pair, validating that both are finite numbers.
fn parse_coordinates(raw: Option<&str>) -> anyhow::Result<Option<(f64, f64)>> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let (lat, lng) = raw
        .split_once(',')
        .context("--coordinates must be given as LAT,LNG")?;
    let lat: f64 = lat.trim().parse().context("latitude is not a number")?;
    let lng: f64 = lng.trim().parse().context("longitude is not a number")?;
    anyhow::ensure!(
        lat.is_finite() && lng.is_finite(),
        "coordinates must be finite"
    );
    Ok(Some((lat, lng)))
}

#[cfg(test)]
mod tests {
    use super::parse_coordinates;

    #[test]
    fn absent_coordinates_stay_absent() {
        // Given no --coordinates flag.
        // When parsed.
        // Then nothing is configured, leaving the upstream default in place.
        assert!(parse_coordinates(None).expect("no flag is valid").is_none());
    }

    #[test]
    fn parses_a_latitude_longitude_pair() {
        // Given a well-formed pair.
        // When parsed.
        // Then both components are returned as floats.
        let parsed = parse_coordinates(Some("22.3364,114.1796")).expect("valid pair");
        assert_eq!(parsed, Some((22.3364, 114.1796)));
    }

    #[test]
    fn rejects_a_malformed_pair() {
        // Given malformed inputs.
        // When parsed.
        // Then each is rejected instead of silently defaulting.
        assert!(parse_coordinates(Some("22.3364")).is_err());
        assert!(parse_coordinates(Some("north,114")).is_err());
        assert!(parse_coordinates(Some("inf,0")).is_err());
    }
}
