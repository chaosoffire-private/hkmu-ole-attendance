//! Configuration loaded from the environment, with typed newtypes.

use jiff::tz::TimeZone;

use crate::domain::time::TimeOfDay;
use crate::error::{AppError, Result};
use crate::secret::Secret;

/// Default OLE entry point.
pub const DEFAULT_OLE_URL: &str = "https://iole.hkmu.edu.hk";
/// NAM CIDP endpoint that accepts the credential POST.
pub const DEFAULT_NAM_LOGIN_URL: &str = "https://auth.hkmu.edu.hk/nidp/app/login?sid=0&sid=0";
/// Express API host serving `getTodayClass`.
pub const DEFAULT_OLECONNECT_API_URL: &str =
    "https://oleconnect.hkmu.edu.hk/oledb/api/getTodayClass/";
/// Default timezone: the university, and therefore the class timetable, is in HKT.
pub const DEFAULT_TIMEZONE: &str = "Asia/Hong_Kong";
/// Default daily setup time (HKT), matching the historical Python behaviour.
pub const DEFAULT_SCHEDULE_TIME: &str = "03:00";

/// How the client authenticates against OLE.
#[derive(Debug, Clone)]
pub enum Credentials {
    /// A pre-captured `Cookie:` header, used verbatim for hkmu hosts.
    SessionCookie(Secret),
    /// `STUDENT_ID` / `STUDENT_PASSWORD` for the SSO chain.
    Password {
        /// Student identifier used as the SSO username.
        student_id: Secret,
        /// Student password used as the SSO credential.
        password: Secret,
    },
}

impl Credentials {
    /// Short label for logs, never containing secret material.
    pub const fn label(&self) -> &'static str {
        match self {
            Self::SessionCookie(_) => "session-cookie",
            Self::Password { .. } => "password-sso",
        }
    }
}

/// Fully resolved runtime configuration.
#[derive(Debug, Clone)]
pub struct Config {
    /// Base OLE URL (the SSO-protected portal).
    pub ole_url: String,
    /// NAM credential endpoint.
    pub nam_login_url: String,
    /// `getTodayClass` API endpoint.
    pub oleconnect_api_url: String,
    /// Discord webhook, when notifications are enabled.
    pub discord_webhook: Option<Secret>,
    /// Wall-clock time to run the daily setup.
    pub schedule_time: TimeOfDay,
    /// Timezone used for all scheduling and class-time comparisons.
    pub timezone: TimeZone,
    /// Timezone name as configured, for logging.
    pub timezone_name: String,
    /// Authentication material.
    pub credentials: Credentials,
    /// Poll interval between attendance checks.
    pub attendance_poll_interval: std::time::Duration,
    /// Emit the "almost over" warning once this much time remains.
    pub warning_threshold: std::time::Duration,
    /// Assumed class length when the API omits an end time.
    pub default_class_duration: std::time::Duration,
}

/// Read an environment variable, treating blank values as unset.
fn env_opt(key: &str) -> Option<String> {
    match std::env::var(key) {
        Ok(value) if !value.trim().is_empty() => Some(value.trim().to_owned()),
        _ => None,
    }
}

/// Read an environment variable with a fallback default.
fn env_or(key: &str, default: &str) -> String {
    env_opt(key).unwrap_or_else(|| default.to_owned())
}

impl Config {
    /// Build the configuration from the current process environment.
    ///
    /// `SESSION_COOKIE` takes precedence over `STUDENT_ID`/`STUDENT_PASSWORD`.
    ///
    /// # Errors
    /// Returns [`AppError::Config`] when neither credential form is present or
    /// a supplied value cannot be parsed.
    pub fn from_env() -> Result<Self> {
        let session_cookie = env_opt("SESSION_COOKIE").map(Secret::new);
        let student_id = env_opt("STUDENT_ID").map(Secret::new);
        let student_password = env_opt("STUDENT_PASSWORD").map(Secret::new);

        let credentials = match (session_cookie, student_id, student_password) {
            (Some(cookie), _, _) => Credentials::SessionCookie(cookie),
            (None, Some(student_id), Some(password)) => Credentials::Password {
                student_id,
                password,
            },
            (None, None, None) => {
                return Err(AppError::Config(
                    "no credentials: set SESSION_COOKIE, or both STUDENT_ID and STUDENT_PASSWORD"
                        .to_owned(),
                ));
            }
            (None, Some(_), None) => {
                return Err(AppError::Config(
                    "STUDENT_ID is set but STUDENT_PASSWORD is missing".to_owned(),
                ));
            }
            (None, None, Some(_)) => {
                return Err(AppError::Config(
                    "STUDENT_PASSWORD is set but STUDENT_ID is missing".to_owned(),
                ));
            }
        };

        let timezone_name = env_or("TIMEZONE", DEFAULT_TIMEZONE);
        let timezone = TimeZone::get(&timezone_name).map_err(|error| {
            AppError::Config(format!("unknown TIMEZONE {timezone_name:?}: {error}"))
        })?;

        Ok(Self {
            ole_url: env_or("OLE_URL", DEFAULT_OLE_URL),
            nam_login_url: env_or("NAM_LOGIN_URL", DEFAULT_NAM_LOGIN_URL),
            oleconnect_api_url: env_or("OLECONNECT_API_URL", DEFAULT_OLECONNECT_API_URL),
            discord_webhook: env_opt("DISCORD_WEBHOOK").map(Secret::new),
            schedule_time: TimeOfDay::parse(&env_or("SCHEDULE_TIME", DEFAULT_SCHEDULE_TIME))?,
            timezone,
            timezone_name,
            credentials,
            attendance_poll_interval: std::time::Duration::from_secs(600),
            warning_threshold: std::time::Duration::from_secs(30 * 60),
            default_class_duration: std::time::Duration::from_secs(3 * 60 * 60),
        })
    }
}

#[cfg(test)]
mod tests {
    use jiff::tz::TimeZone;

    use super::{DEFAULT_TIMEZONE, TimeOfDay};

    #[test]
    fn parses_zero_padded_time() {
        // Given a zero-padded HH:MM value.
        // When parsed.
        let parsed = TimeOfDay::parse("03:00").expect("03:00 is valid");
        // Then the components match.
        assert_eq!((parsed.hour(), parsed.minute()), (3, 0));
    }

    #[test]
    fn parses_single_digit_hour() {
        // Given a single-digit hour.
        // When parsed.
        let parsed = TimeOfDay::parse("3:05").expect("3:05 is valid");
        // Then it is normalised to the same instant.
        assert_eq!((parsed.hour(), parsed.minute()), (3, 5));
        assert_eq!(parsed.to_string(), "03:05");
    }

    #[test]
    fn strips_inline_comment_from_value() {
        // Given a value polluted by a docker --env-file inline comment.
        // When parsed.
        let parsed = TimeOfDay::parse("03:00 # HKT daily").expect("comment is tolerated");
        // Then the comment is ignored.
        assert_eq!(parsed.to_string(), "03:00");
    }

    #[test]
    fn rejects_out_of_range_time() {
        // Given times outside a 24-hour day.
        // When parsed.
        // Then both are rejected.
        assert!(TimeOfDay::parse("24:00").is_err());
        assert!(TimeOfDay::parse("12:60").is_err());
    }

    #[test]
    fn rejects_non_numeric_time() {
        // Given a non-numeric value.
        // When parsed.
        // Then it is rejected rather than silently defaulting.
        assert!(TimeOfDay::parse("noon").is_err());
    }

    #[test]
    fn hong_kong_is_a_known_timezone() {
        // Given the documented default timezone name.
        // When looked up in the tz database.
        // Then it resolves, so the default can never be invalid.
        assert!(TimeZone::get(DEFAULT_TIMEZONE).is_ok());
    }
}
