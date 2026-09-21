//! Wall-clock values and the arithmetic the scheduler needs.
//!
//! Everything here is a pure function over values: no clock is read, no lock is
//! taken, and nothing is awaited. The current instant is always supplied by the
//! caller, which is what makes the decision logic testable.

use std::fmt;
use std::time::Duration;

use jiff::civil::{Date, DateTime};
use jiff::tz::TimeZone;
use jiff::{Span, Zoned};

use super::error::DomainError;

/// The wire format the OLE API uses for class start and end times.
pub const CLASS_TIME_FORMAT: &str = "%Y-%m-%d %H:%M";

/// A `HH:MM` wall-clock time of day.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeOfDay {
    hour: u8,
    minute: u8,
}

impl TimeOfDay {
    /// Parse `H:MM` or `HH:MM`.
    ///
    /// A trailing `#` comment is tolerated because `docker --env-file` keeps
    /// inline comments inside the value.
    ///
    /// # Errors
    /// Returns [`DomainError::TimeOfDay`] when the text is not a valid time.
    pub fn parse(raw: &str) -> Result<Self, DomainError> {
        let value = raw.split('#').next().unwrap_or(raw).trim();
        let reject = |reason: &'static str| DomainError::TimeOfDay {
            raw: raw.to_owned(),
            reason,
        };

        let (hours, minutes) = value
            .split_once(':')
            .ok_or_else(|| reject("expected HH:MM"))?;
        let hour: u8 = hours.trim().parse().map_err(|_| reject("bad hour"))?;
        let minute: u8 = minutes.trim().parse().map_err(|_| reject("bad minute"))?;
        if hour > 23 || minute > 59 {
            return Err(reject("out of range"));
        }
        Ok(Self { hour, minute })
    }

    /// Hour component, 0-23.
    pub const fn hour(self) -> u8 {
        self.hour
    }

    /// Minute component, 0-59.
    pub const fn minute(self) -> u8 {
        self.minute
    }
}

impl fmt::Display for TimeOfDay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:02}:{:02}", self.hour, self.minute)
    }
}

/// Parse the OLE `YYYY-MM-DD HH:MM` form as civil (wall-clock) time.
///
/// # Errors
/// Returns [`DomainError::Timestamp`] when the text does not match the format.
pub fn parse_class_time(raw: &str) -> Result<DateTime, DomainError> {
    DateTime::strptime(CLASS_TIME_FORMAT, raw).map_err(|_| DomainError::Timestamp {
        raw: raw.to_owned(),
    })
}

/// The fallback class length as a span.
pub fn duration_span(duration: Duration) -> Span {
    let minutes = i64::try_from(duration.as_secs() / 60).unwrap_or(180);
    Span::new().minutes(minutes)
}

/// End time to use when the API supplied none.
///
/// Falls back to the start time if the addition overflows the representable
/// date range, so an absurd configured duration cannot abort the poll.
pub fn fallback_end(starts_at: DateTime, duration: Duration) -> DateTime {
    starts_at
        .checked_add(duration_span(duration))
        .unwrap_or(starts_at)
}

/// The effective end of a class, resolved into a zoned instant.
///
/// Returns `None` when the civil time cannot be placed in `zone`, which only
/// happens for a civil time that does not exist in that zone.
pub fn class_end(
    starts_at: DateTime,
    ends_at: Option<DateTime>,
    fallback: Duration,
    zone: &TimeZone,
) -> Option<Zoned> {
    ends_at
        .unwrap_or_else(|| fallback_end(starts_at, fallback))
        .to_zoned(zone.clone())
        .ok()
}

/// Whole minutes from `now` until `target`, saturating at zero.
pub fn minutes_until(target: &Zoned, now: &Zoned) -> i64 {
    target
        .timestamp()
        .as_second()
        .saturating_sub(now.timestamp().as_second())
        .max(0)
        .checked_div(60)
        .unwrap_or(0)
}

/// Seconds from `now` until the next occurrence of `at`.
///
/// When `at` has already passed today the next day's occurrence is used, so the
/// result is always positive and the caller cannot spin.
pub fn seconds_until(at: TimeOfDay, now: &Zoned) -> i64 {
    let hour = i8::try_from(at.hour()).unwrap_or(0);
    let minute = i8::try_from(at.minute()).unwrap_or(0);
    let candidate = |date: Date| {
        date.at(hour, minute, 0, 0)
            .to_zoned(now.time_zone().clone())
            .ok()
    };

    let mut target = candidate(now.date());
    if target
        .as_ref()
        .is_none_or(|value| value.timestamp() <= now.timestamp())
    {
        if let Ok(tomorrow) = now.date().tomorrow() {
            if let Some(next) = candidate(tomorrow) {
                target = Some(next);
            }
        }
    }

    target.map_or(1, |value| {
        value
            .timestamp()
            .as_second()
            .saturating_sub(now.timestamp().as_second())
            .max(1)
    })
}

/// Waiting period before the next occurrence of `at`.
pub fn duration_until_next(at: TimeOfDay, now: &Zoned) -> Duration {
    Duration::from_secs(u64::try_from(seconds_until(at, now)).unwrap_or(1))
}

#[cfg(test)]
mod tests {
    use jiff::civil::date;
    use jiff::tz::TimeZone;

    use super::{
        TimeOfDay, class_end, duration_until_next, fallback_end, minutes_until, parse_class_time,
        seconds_until,
    };

    fn hkt(stamp: &str) -> jiff::Zoned {
        stamp
            .parse::<jiff::Timestamp>()
            .expect("valid timestamp")
            .to_zoned(TimeZone::get("Asia/Hong_Kong").expect("valid tz"))
    }

    #[test]
    fn parses_zero_padded_time() {
        // Given a zero-padded HH:MM value.
        // When parsed.
        let parsed = TimeOfDay::parse("03:00").expect("03:00 is valid");

        // Then the components match.
        assert_eq!((parsed.hour(), parsed.minute()), (3, 0));
        assert_eq!(parsed.to_string(), "03:00");
    }

    #[test]
    fn parses_single_digit_hour_and_strips_a_comment() {
        // Given a single-digit hour and a docker inline comment.
        // When parsed.
        let parsed = TimeOfDay::parse("3:05 # HKT daily").expect("tolerated");

        // Then the comment is ignored and the value normalises.
        assert_eq!(parsed.to_string(), "03:05");
    }

    #[test]
    fn rejects_out_of_range_and_non_numeric_times() {
        // Given values outside a 24-hour day and a non-numeric one.
        // When parsed.
        // Then each is rejected rather than silently defaulting.
        assert!(TimeOfDay::parse("24:00").is_err());
        assert!(TimeOfDay::parse("12:60").is_err());
        assert!(TimeOfDay::parse("noon").is_err());
    }

    #[test]
    fn parses_the_ole_timestamp_format() {
        // Given a timestamp in the OLE wire format.
        // When parsed.
        let parsed = parse_class_time("2026-09-21 09:00").expect("valid");

        // Then the civil components match.
        assert_eq!(parsed.date(), date(2026, 9, 21));
        assert_eq!((parsed.hour(), parsed.minute()), (9, 0));
    }

    #[test]
    fn rejects_a_malformed_timestamp() {
        // Given garbage and empty input.
        // When parsed.
        // Then both are rejected.
        assert!(parse_class_time("not a time").is_err());
        assert!(parse_class_time("").is_err());
    }

    #[test]
    fn fallback_end_uses_the_configured_duration() {
        // Given a session with no end time and a three-hour default.
        let start = date(2026, 9, 21).at(9, 0, 0, 0);

        // When the fallback end is computed.
        let end = fallback_end(start, std::time::Duration::from_secs(3 * 3600));

        // Then it is exactly three hours later.
        assert_eq!(end, date(2026, 9, 21).at(12, 0, 0, 0));
    }

    #[test]
    fn class_end_prefers_the_supplied_end_time() {
        // Given a session that does supply an end time.
        let zone = TimeZone::get("Asia/Hong_Kong").expect("valid tz");
        let start = date(2026, 9, 21).at(9, 0, 0, 0);
        let end = date(2026, 9, 21).at(9, 50, 0, 0);

        // When the effective end is resolved.
        let resolved = class_end(
            start,
            Some(end),
            std::time::Duration::from_secs(10800),
            &zone,
        )
        .expect("resolves in this zone");

        // Then the supplied end wins over the fallback.
        assert_eq!(resolved.datetime(), end);
    }

    #[test]
    fn class_end_falls_back_when_no_end_time_is_supplied() {
        // Given a session with no end time.
        let zone = TimeZone::get("Asia/Hong_Kong").expect("valid tz");
        let start = date(2026, 9, 21).at(9, 0, 0, 0);

        // When the effective end is resolved.
        let resolved = class_end(start, None, std::time::Duration::from_secs(10800), &zone)
            .expect("resolves in this zone");

        // Then the three-hour fallback is used.
        assert_eq!(resolved.datetime(), date(2026, 9, 21).at(12, 0, 0, 0));
    }

    #[test]
    fn targets_later_the_same_day_when_the_time_is_still_ahead() {
        // Given 02:00 HKT and a 03:00 schedule.
        let now = hkt("2026-09-21T02:00:00+08:00");

        // When the wait is computed.
        let wait = duration_until_next(TimeOfDay::parse("03:00").expect("valid"), &now);

        // Then it is exactly one hour.
        assert_eq!(wait.as_secs(), 3600);
    }

    #[test]
    fn rolls_over_to_tomorrow_when_the_time_has_passed() {
        // Given 04:00 HKT and a 03:00 schedule that already elapsed today.
        let now = hkt("2026-09-21T04:00:00+08:00");

        // When the wait is computed.
        let wait = duration_until_next(TimeOfDay::parse("03:00").expect("valid"), &now);

        // Then it is 23 hours, not a zero-length or negative wait.
        assert_eq!(wait.as_secs(), 23 * 3600);
    }

    #[test]
    fn never_returns_a_zero_wait_at_the_exact_schedule_minute() {
        // Given the clock sitting exactly on the schedule time.
        let now = hkt("2026-09-21T03:00:00+08:00");

        // When the wait is computed.
        let seconds = seconds_until(TimeOfDay::parse("03:00").expect("valid"), &now);

        // Then it is positive, so the loop cannot spin.
        assert!(seconds > 0, "expected a positive wait, got {seconds}");
    }

    #[test]
    fn minutes_until_saturates_and_reports_whole_minutes() {
        // Given a target that passed, and one 90 minutes ahead.
        let now = hkt("2026-09-21T10:00:00+08:00");
        let past = hkt("2026-09-21T09:00:00+08:00");
        let later = hkt("2026-09-21T11:30:00+08:00");

        // When the remaining minutes are computed.
        // Then it clamps at zero and otherwise reports whole minutes.
        assert_eq!(minutes_until(&past, &now), 0);
        assert_eq!(minutes_until(&later, &now), 90);
    }

    #[test]
    fn respects_the_timezone_it_is_given() {
        // Given the same instant observed in two zones with a 00:30 schedule.
        let hk = hkt("2026-09-21T00:00:00+08:00");
        let utc = hk.timestamp().to_zoned(TimeZone::UTC);
        let at = TimeOfDay::parse("00:30").expect("valid");

        // When the wait is computed in each zone.
        // Then the wall-clock schedule is interpreted per zone, so they differ.
        assert_ne!(seconds_until(at, &hk), seconds_until(at, &utc));
    }
}
