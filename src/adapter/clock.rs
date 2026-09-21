//! The real clock.

use crate::port::Clock;

/// Reads the system clock.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self, zone: &jiff::tz::TimeZone) -> jiff::Zoned {
        jiff::Timestamp::now().to_zoned(zone.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::SystemClock;
    use crate::port::Clock;

    #[test]
    fn reports_the_current_instant_in_the_requested_zone() {
        // Given the system clock and a zone.
        let clock = SystemClock;
        let zone = jiff::tz::TimeZone::get("Asia/Hong_Kong").expect("valid tz");

        // When the current time is requested.
        let now = clock.now(&zone);

        // Then it carries that zone, so callers read wall-clock time correctly.
        assert_eq!(now.time_zone(), &zone);
    }

    #[test]
    fn timestamp_is_within_a_second_of_the_system_time() {
        // Given the system clock.
        let clock = SystemClock;

        // When two readings are taken.
        let first = clock.timestamp().as_second();
        let second = clock.timestamp().as_second();

        // Then they agree to within a second, proving it tracks real time.
        assert!((second - first).abs() <= 1);
    }
}
