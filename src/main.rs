//! Entry point: parse arguments, load configuration, and run.

use anyhow::Context as _;
use clap::Parser;
use hkmu_ole_attendance::app::{App, Report};
use hkmu_ole_attendance::config::Config;
use hkmu_ole_attendance::domain::geo::Coordinates;
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
    let app = App::new(config, coordinates);

    if cli.probe {
        return app.probe().await.context("probing attendance activities");
    }
    if cli.fetch_only || cli.notify_only {
        let report = if cli.notify_only {
            Report::Discord
        } else {
            Report::Log
        };
        return app.fetch_once(report).await.context("running daily setup");
    }
    app.run_scheduler().await;
    Ok(())
}

/// Parse a `lat,lng` pair, rejecting anything that is not two finite numbers.
fn parse_coordinates(raw: Option<&str>) -> anyhow::Result<Option<Coordinates>> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let (lat, lng) = raw
        .split_once(',')
        .context("--coordinates must be given as LAT,LNG")?;
    let lat: f64 = lat.trim().parse().context("latitude is not a number")?;
    let lng: f64 = lng.trim().parse().context("longitude is not a number")?;
    Ok(Some(Coordinates::new(lat, lng)?))
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
        // Then both components are returned in the order given.
        let parsed = parse_coordinates(Some("22.3364,114.1796")).expect("valid pair");
        let point = parsed.expect("a pair was supplied");
        assert!((point.latitude() - 22.3364).abs() < f64::EPSILON);
        assert!((point.longitude() - 114.1796).abs() < f64::EPSILON);
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
