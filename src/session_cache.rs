//! Caching authenticator: reuse an unexpired session, re-login only when needed.
//!
//! Logging in on every attendance poll would hammer the SSO chain with a
//! six-hop round trip and look far more like a bot than a student's browser.
//! Instead the session is held in memory and reused for as long as its
//! `LtpaToken` remains valid; only once it is near expiry does the client
//! perform a fresh login.

use std::sync::Arc;

use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use crate::auth::Authenticator;
use crate::config::Config;
use crate::error::Result;
use crate::ltpa::parse_ltpa_expiry;
use crate::session::Session;

/// Re-login once the cached session has less than this left.
///
/// A margin is needed because a poll may begin just before expiry and still be
/// in flight when it lapses.
pub const REFRESH_MARGIN_SECS: i64 = 5 * 60;

/// Holds one authenticated session and refreshes it when it ages out.
///
/// Cheap to clone: the slots are shared, so every clone observes the same
/// cached session and an invalidation by one is seen by all.
#[derive(Debug, Clone)]
pub struct SessionCache {
    config: Arc<Config>,
    cached: Arc<Mutex<Option<Session>>>,
}

impl SessionCache {
    /// Create an empty cache for the given configuration.
    pub fn new(config: Config) -> Self {
        Self {
            config: Arc::new(config),
            cached: Arc::new(Mutex::new(None)),
        }
    }

    /// Return a usable session, reusing the cached one while it is valid.
    ///
    /// # Errors
    /// Returns the underlying authentication error when a fresh login is
    /// required and fails.
    pub async fn session(&self) -> Result<Session> {
        let now = jiff::Timestamp::now().as_second();

        // Inspect the cached session and release the lock before any await.
        // Holding it across the login would serialise every concurrent poll
        // behind one six-hop round trip.
        let cached = {
            let mut guard = self.cached.lock().await;
            match guard.as_ref() {
                Some(existing) => {
                    let expiry = Self::expiry_of(existing);
                    match expiry {
                        Some(expiry)
                            if !expiry.is_expired(now.saturating_add(REFRESH_MARGIN_SECS)) =>
                        {
                            debug!(
                                remaining_secs = expiry.remaining_secs(now),
                                "reusing cached session"
                            );
                            return Ok(existing.clone());
                        }
                        Some(expiry) => {
                            info!(
                                remaining_secs = expiry.remaining_secs(now),
                                "cached session is at end of life; re-authenticating"
                            );
                            guard.take().is_some()
                        }
                        None => {
                            debug!("cached session carries no readable expiry; reusing it");
                            return Ok(existing.clone());
                        }
                    }
                }
                None => false,
            }
        };
        debug!(discarded = cached, "cache slot cleared for refresh");

        let fresh = Authenticator::new((*self.config).clone())
            .authenticate()
            .await?;
        *self.cached.lock().await = Some(fresh.clone());
        Ok(fresh)
    }

    /// Drop the cached session so the next call must log in again.
    ///
    /// Used when the server rejects a session that still looked valid locally.
    pub async fn invalidate(&self) {
        let mut guard = self.cached.lock().await;
        if guard.take().is_some() {
            debug!("cached session discarded");
        }
    }

    /// Read the expiry embedded in a session's `LtpaToken`, if present.
    fn expiry_of(session: &Session) -> Option<crate::ltpa::TokenExpiry> {
        let token = session.cookie_value("LtpaToken")?;
        let expiry = parse_ltpa_expiry(token);
        if expiry.is_none() {
            warn!("could not read the expiry from LtpaToken; will reuse it as-is");
        }
        expiry
    }
}

#[cfg(test)]
mod tests {
    use super::{REFRESH_MARGIN_SECS, SessionCache};
    use crate::config::{Config, Credentials};
    use crate::domain::time::TimeOfDay;
    use crate::ltpa::TokenExpiry;
    use crate::secret::Secret;

    fn config() -> Config {
        Config {
            ole_url: "https://iole.hkmu.edu.hk".to_owned(),
            nam_login_url: String::new(),
            oleconnect_api_url: String::new(),
            discord_webhook: None,
            schedule_time: TimeOfDay::parse("03:00").expect("valid"),
            timezone: jiff::tz::TimeZone::get("Asia/Hong_Kong").expect("valid tz"),
            timezone_name: "Asia/Hong_Kong".to_owned(),
            credentials: Credentials::SessionCookie(Secret::new("LtpaToken=x")),
            attendance_poll_interval: std::time::Duration::from_secs(600),
            warning_threshold: std::time::Duration::from_secs(1800),
            default_class_duration: std::time::Duration::from_secs(10800),
        }
    }

    #[tokio::test]
    async fn a_fresh_cache_reports_no_cached_session() {
        // Given an empty cache.
        let cache = SessionCache::new(config());

        // When the cached slot is inspected.
        // Then it is empty, so the first call must authenticate.
        assert!(cache.cached.lock().await.is_none());
    }

    #[tokio::test]
    async fn invalidate_clears_the_cached_session() {
        // Given a cache holding a session.
        let cache = SessionCache::new(config());
        *cache.cached.lock().await = Some(crate::session::Session::new().expect("client builds"));

        // When it is invalidated.
        cache.invalidate().await;

        // Then the slot is empty again, forcing a fresh login next time.
        assert!(cache.cached.lock().await.is_none());
    }

    #[test]
    fn refresh_margin_is_positive_and_shorter_than_a_token_lifetime() {
        // Given the configured margin.
        // When compared with the four-hour token lifetime.
        // Then it is a real safety margin that cannot cause needless logins.
        let margin = REFRESH_MARGIN_SECS;
        let token_lifetime = 14_400_i64;
        assert!(margin > 0);
        assert!(margin < token_lifetime);
    }

    #[test]
    fn margin_decision_boundary_is_exclusive_at_exactly_the_margin() {
        // Given a token expiring exactly REFRESH_MARGIN_SECS from now.
        let now = 1_000_000_i64;
        let expiry = TokenExpiry {
            issued_at: now,
            expires_at: now + REFRESH_MARGIN_SECS,
        };

        // When tested against the refresh condition.
        // Then it is treated as due for refresh, so a poll cannot outlive it.
        assert!(expiry.is_expired(now + REFRESH_MARGIN_SECS));

        // And a token with even one more second is still usable.
        let healthy = TokenExpiry {
            issued_at: now,
            expires_at: now + REFRESH_MARGIN_SECS + 1,
        };
        assert!(!healthy.is_expired(now + REFRESH_MARGIN_SECS));
    }

    /// Build a session whose `LtpaToken` expires `offset_from_now_secs` from now.
    ///
    /// The token is shaped like a real one, with the issue time in the past, so
    /// that a negative offset still produces a well-formed (but expired) token.
    fn session_with_expiry(offset_from_now_secs: i64) -> crate::session::Session {
        use base64::Engine as _;
        use base64::engine::general_purpose::STANDARD;

        let expires_at = jiff::Timestamp::now()
            .as_second()
            .saturating_add(offset_from_now_secs);
        let issued_at = expires_at.saturating_sub(14_400);

        let issue = u32::try_from(issued_at).expect("fits in u32");
        let expiry = u32::try_from(expires_at).expect("fits in u32");

        let mut raw = vec![0x00, 0x01, 0x02, 0x03];
        raw.extend_from_slice(format!("{issue:08X}").as_bytes());
        raw.extend_from_slice(format!("{expiry:08X}").as_bytes());
        raw.extend_from_slice(b"CN=s1234567/OU=OUHKStudent/O=ouhk");
        let token = STANDARD.encode(raw);

        let mut session = crate::session::Session::new().expect("client builds");
        session.cookies_mut().upsert(crate::cookie::Cookie {
            name: "LtpaToken".to_owned(),
            value: token,
            domain: ".hkmu.edu.hk".to_owned(),
            path: "/".to_owned(),
            secure: false,
        });
        session
    }

    /// A session whose token cannot be parsed at all.
    fn session_with_opaque_token() -> crate::session::Session {
        let mut session = crate::session::Session::new().expect("client builds");
        session.cookies_mut().upsert(crate::cookie::Cookie {
            name: "LtpaToken".to_owned(),
            value: "not-a-real-token".to_owned(),
            domain: ".hkmu.edu.hk".to_owned(),
            path: "/".to_owned(),
            secure: false,
        });
        session
    }

    #[tokio::test]
    async fn a_long_lived_cached_session_is_reused_across_many_calls() {
        // Given a cache holding a session whose token is valid for hours, and a
        // configuration that could not possibly log in (empty endpoints).
        let cache = SessionCache::new(config());
        *cache.cached.lock().await = Some(session_with_expiry(4 * 3600));

        // When the session is requested repeatedly, as the attendance poller
        // does every ten minutes for the length of a class.
        for _ in 0..8 {
            let returned = cache
                .session()
                .await
                .expect("a valid cached session needs no network");

            // Then every call returns the cached token untouched. A login
            // attempt would fail, because the config names no real endpoints,
            // so success here proves the cache served every call.
            assert_eq!(
                returned.cookie_value("LtpaToken"),
                cache
                    .cached
                    .lock()
                    .await
                    .as_ref()
                    .and_then(|s| s.cookie_value("LtpaToken"))
            );
        }
    }

    #[tokio::test]
    async fn an_expired_cached_session_is_not_reused() {
        // Given a cache holding an already-expired session.
        let cache = SessionCache::new(config());
        *cache.cached.lock().await = Some(session_with_expiry(-60));

        // When a session is requested.
        let outcome = cache.session().await;

        // Then the cache refuses to hand back the dead session and instead
        // attempts a fresh login, which fails here because the test config
        // points at no real endpoints. The error proves refresh was attempted.
        assert!(outcome.is_err(), "expired session must not be returned");
    }

    #[tokio::test]
    async fn a_session_inside_the_margin_is_treated_as_expired() {
        // Given a cache whose token expires inside the refresh margin.
        let cache = SessionCache::new(config());
        *cache.cached.lock().await = Some(session_with_expiry(REFRESH_MARGIN_SECS - 1));

        // When a session is requested.
        let outcome = cache.session().await;

        // Then it refreshes early rather than risking a poll outliving it.
        assert!(outcome.is_err(), "session inside the margin must refresh");
    }

    #[tokio::test]
    async fn a_session_just_outside_the_margin_is_reused() {
        // Given a token that outlives the margin by a comfortable step.
        let cache = SessionCache::new(config());
        *cache.cached.lock().await = Some(session_with_expiry(REFRESH_MARGIN_SECS + 600));

        // When a session is requested.
        let outcome = cache.session().await;

        // Then no login is attempted, so the reuse path succeeded.
        assert!(outcome.is_ok(), "a healthy session must be reused");
    }

    #[tokio::test]
    async fn an_unreadable_token_is_reused_rather_than_forcing_a_login() {
        // Given a cache whose token cannot be parsed, as would happen if the
        // server changed its token format.
        let cache = SessionCache::new(config());
        *cache.cached.lock().await = Some(session_with_opaque_token());

        // When a session is requested.
        let outcome = cache.session().await;

        // Then the session is handed back and the server decides, rather than
        // this client refusing to work with a token it merely cannot read.
        assert!(
            outcome.is_ok(),
            "an opaque token must be attempted, not rejected locally"
        );
    }
}
