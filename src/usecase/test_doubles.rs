//! Scripted test doubles for the ports.
//!
//! Test-only: the panic-related lints are relaxed here because a poisoned lock
//! in a test double should fail loudly, and these helpers never ship.
//!
//! Available to every use-case test: because the use cases depend on traits
//! rather than concrete adapters, a whole attendance run can be exercised with
//! no network, no clock and no waiting.

#![allow(
    clippy::missing_panics_doc,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    reason = "test double: a poisoned lock or bad index should fail the test loudly"
)]

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use jiff::{Timestamp, Zoned};

use crate::domain::attendance::Submission;
use crate::domain::schedule::{ScheduledClass, TodayClassResponse};
use crate::port::error::PortError;
use crate::port::session::CookieHeader;
use crate::port::{AttendanceGateway, Clock, Notice, Notifier, ScheduleGateway, SessionProvider};

/// A clock the test drives by hand.
#[derive(Debug)]
pub struct FakeClock {
    now: Mutex<Zoned>,
}

impl FakeClock {
    /// Start at the given instant.
    pub fn at(now: Zoned) -> Self {
        Self {
            now: Mutex::new(now),
        }
    }

    /// Advance by the given number of seconds.
    pub fn advance_secs(&self, seconds: i64) {
        let mut guard = self.now.lock().expect("clock lock");
        if let Ok(next) = guard
            .clone()
            .checked_add(jiff::Span::new().seconds(seconds))
        {
            *guard = next;
        }
    }
}

impl Clock for FakeClock {
    fn now(&self, _zone: &jiff::tz::TimeZone) -> Zoned {
        self.now.lock().expect("clock lock").clone()
    }

    fn timestamp(&self) -> Timestamp {
        self.now.lock().expect("clock lock").timestamp()
    }
}

/// A notifier that records everything it was told.
#[derive(Debug, Default)]
pub struct RecordingNotifier {
    notices: Mutex<Vec<(Notice, String)>>,
}

impl RecordingNotifier {
    /// Every notice delivered, in order.
    pub fn notices(&self) -> Vec<(Notice, String)> {
        self.notices.lock().expect("notifier lock").clone()
    }

    /// The bodies delivered at a given level.
    pub fn bodies_at(&self, level: Notice) -> Vec<String> {
        self.notices()
            .into_iter()
            .filter(|(seen, _)| *seen == level)
            .map(|(_, body)| body)
            .collect()
    }

    /// Whether any delivered body contains `needle`.
    pub fn contains(&self, needle: &str) -> bool {
        self.notices().iter().any(|(_, body)| body.contains(needle))
    }
}

impl Notifier for RecordingNotifier {
    async fn notify(&self, level: Notice, message: &str) -> Result<(), PortError> {
        self.notices
            .lock()
            .expect("notifier lock")
            .push((level, message.to_owned()));
        Ok(())
    }
}

/// An attendance gateway that replays a scripted sequence of outcomes.
///
/// Optionally advances a [`FakeClock`] on each call, because a poll loop that
/// observes a frozen clock would never leave its window.
#[derive(Debug)]
pub struct ScriptedGateway {
    outcomes: Mutex<VecDeque<Result<Submission, PortError>>>,
    submissions: Mutex<u32>,
    clock: Option<(Arc<FakeClock>, i64)>,
}

impl ScriptedGateway {
    /// Replay `outcomes` in order; the last one repeats once exhausted.
    pub fn new(outcomes: Vec<Result<Submission, PortError>>) -> Self {
        Self {
            outcomes: Mutex::new(outcomes.into()),
            submissions: Mutex::new(0),
            clock: None,
        }
    }

    /// Advance `clock` by `seconds` on every call, simulating elapsed time.
    pub fn advancing(
        outcomes: Vec<Result<Submission, PortError>>,
        clock: Arc<FakeClock>,
        seconds: i64,
    ) -> Self {
        Self {
            outcomes: Mutex::new(outcomes.into()),
            submissions: Mutex::new(0),
            clock: Some((clock, seconds)),
        }
    }

    /// How many times `submit` was called.
    pub fn submission_count(&self) -> u32 {
        *self.submissions.lock().expect("gateway lock")
    }
}

impl AttendanceGateway for ScriptedGateway {
    async fn submit(
        &self,
        _class: &ScheduledClass,
        _coordinates: Option<(f64, f64)>,
    ) -> Result<Submission, PortError> {
        *self.submissions.lock().expect("gateway lock") += 1;
        if let Some((clock, seconds)) = &self.clock {
            clock.advance_secs(*seconds);
        }
        let mut guard = self.outcomes.lock().expect("gateway lock");
        // A match reads more clearly here than the equivalent combinators,
        // whose type inference in this position is genuinely worse.
        #[allow(
            clippy::option_if_let_else,
            reason = "the match is clearer and infers without annotations"
        )]
        match guard.pop_front() {
            Some(outcome) => outcome,
            None => Err(exhausted()),
        }
    }
}

/// A session provider that only counts invalidations.
#[derive(Debug, Default)]
pub struct FakeSession {
    invalidations: Mutex<u32>,
}

impl FakeSession {
    /// How many times the session was invalidated.
    pub fn invalidations(&self) -> u32 {
        *self.invalidations.lock().expect("session lock")
    }
}

impl SessionProvider for FakeSession {
    async fn session(&self) -> Result<CookieHeader, PortError> {
        Ok(CookieHeader::new("fake"))
    }

    async fn invalidate(&self) {
        *self.invalidations.lock().expect("session lock") += 1;
    }
}

/// The error reported once a script runs out of entries.
fn exhausted() -> PortError {
    PortError::Remote("script exhausted".to_owned())
}

/// A schedule gateway returning a fixed payload.
#[derive(Debug)]
pub struct FixedScheduleGateway {
    payload: TodayClassResponse,
}

impl FixedScheduleGateway {
    /// Serve `payload` for every request.
    pub fn new(payload: TodayClassResponse) -> Self {
        Self { payload }
    }
}

impl ScheduleGateway for FixedScheduleGateway {
    async fn today_classes(&self) -> Result<TodayClassResponse, PortError> {
        Ok(self.payload.clone())
    }
}

/// Build a class spanning `start_hour` to `end_hour` on a fixed day.
pub fn class_from(start_hour: i8, end_hour: i8) -> ScheduledClass {
    ScheduledClass {
        termcode: "2604".to_owned(),
        course_code: "ELEC3050SEF".to_owned(),
        name: "Lecture ( Full Time )".to_owned(),
        starts_at: jiff::civil::date(2026, 9, 21).at(start_hour, 0, 0, 0),
        ends_at: Some(jiff::civil::date(2026, 9, 21).at(end_hour, 0, 0, 0)),
        group: "L01".to_owned(),
        venue: "HKMU C0G01".to_owned(),
    }
}

/// A zoned instant in Hong Kong time.
pub fn hkt(stamp: &str) -> Zoned {
    stamp
        .parse::<Timestamp>()
        .expect("valid timestamp")
        .to_zoned(jiff::tz::TimeZone::get("Asia/Hong_Kong").expect("valid tz"))
}
