//! Parsing the expiry embedded in an `LtpaToken`.
//!
//! The token is base64 over a fixed structure:
//!
//! ```text
//! 00 01 02 03 | "6AB115AC" | "6AB14DEC" | CN=.../OU=.../O=... | signature
//! version       issue time   expiry time  user DN               signature
//! ```
//!
//! Both timestamps are ASCII hexadecimal seconds since the Unix epoch. The
//! expiry is written when the token is issued and is absolute — polling the
//! server does not move it, so the only way to extend a session is to log in
//! again.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;

/// LTPA token version marker, four leading bytes.
const VERSION: [u8; 4] = [0x00, 0x01, 0x02, 0x03];
/// Byte range holding the ASCII-hex issue time.
const ISSUE_RANGE: std::ops::Range<usize> = 4..12;
/// Byte range holding the ASCII-hex expiry time.
const EXPIRY_RANGE: std::ops::Range<usize> = 12..20;

/// The timestamps carried inside an LTPA token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokenExpiry {
    /// When the token was issued, in seconds since the Unix epoch.
    pub issued_at: i64,
    /// When the token stops being accepted, in seconds since the Unix epoch.
    pub expires_at: i64,
}

impl TokenExpiry {
    /// Seconds remaining until expiry, relative to `now`, saturating at zero.
    pub fn remaining_secs(&self, now: i64) -> i64 {
        self.expires_at.saturating_sub(now).max(0)
    }

    /// Whether the token is already expired at `now`.
    pub const fn is_expired(&self, now: i64) -> bool {
        now >= self.expires_at
    }
}

/// Decode the issue and expiry timestamps carried by a base64 `LtpaToken`.
///
/// Returns `None` when the value is not a well-formed LTPA token, so the
/// caller can fall back to attempting the request anyway rather than refusing
/// to work with a token it simply does not understand.
pub fn parse_ltpa_expiry(token: &str) -> Option<TokenExpiry> {
    let token = token.trim();
    let raw = STANDARD.decode(token).ok()?;

    if raw.get(..4)? != VERSION {
        return None;
    }
    let issued_at = parse_hex_seconds(raw.get(ISSUE_RANGE)?)?;
    let expires_at = parse_hex_seconds(raw.get(EXPIRY_RANGE)?)?;
    if expires_at < issued_at {
        return None;
    }
    Some(TokenExpiry {
        issued_at,
        expires_at,
    })
}

/// Read eight ASCII hexadecimal digits as seconds.
fn parse_hex_seconds(bytes: &[u8]) -> Option<i64> {
    let text = std::str::from_utf8(bytes).ok()?;
    i64::from_str_radix(text, 16).ok()
}

#[cfg(test)]
mod tests {
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD;

    use super::{TokenExpiry, parse_ltpa_expiry};

    /// Build a token with the same layout as the live one.
    fn token_with(issue: u32, expiry: u32) -> String {
        let mut raw = vec![0x00, 0x01, 0x02, 0x03];
        raw.extend_from_slice(format!("{issue:08X}").as_bytes());
        raw.extend_from_slice(format!("{expiry:08X}").as_bytes());
        raw.extend_from_slice(b"CN=s1234567/OU=OUHKStudent/O=ouhk");
        raw.extend_from_slice(&[0xA1, 0xEE, 0xFA, 0x80]);
        STANDARD.encode(raw)
    }

    #[test]
    fn decodes_the_timestamps_from_a_live_shaped_token() {
        // Given a token shaped exactly like the one the server issues, with the
        // issue/expiry pair observed live (a four hour window).
        let issue = 0x6AB1_15AC_u32;
        let expiry = 0x6AB1_4DEC_u32;
        let encoded = token_with(issue, expiry);

        // When the expiry is parsed.
        let parsed = parse_ltpa_expiry(&encoded).expect("well-formed token");

        // Then both timestamps match and the window is four hours.
        assert_eq!(parsed.issued_at, i64::from(issue));
        assert_eq!(parsed.expires_at, i64::from(expiry));
        assert_eq!(parsed.expires_at - parsed.issued_at, 14_400);
    }

    #[test]
    fn reports_the_remaining_lifetime() {
        // Given a parsed token with a known expiry.
        let parsed = TokenExpiry {
            issued_at: 1_000,
            expires_at: 2_000,
        };

        // When queried at points before and after expiry.
        // Then the remainder shrinks and saturates rather than going negative.
        assert_eq!(parsed.remaining_secs(1_000), 1_000);
        assert_eq!(parsed.remaining_secs(1_500), 500);
        assert_eq!(parsed.remaining_secs(2_000), 0);
        assert_eq!(parsed.remaining_secs(9_999), 0);
    }

    #[test]
    fn detects_expiry_inclusively() {
        // Given a token expiring at t=2000.
        let parsed = TokenExpiry {
            issued_at: 1_000,
            expires_at: 2_000,
        };

        // When checked either side of the boundary.
        // Then the boundary instant itself counts as expired.
        assert!(!parsed.is_expired(1_999));
        assert!(parsed.is_expired(2_000));
        assert!(parsed.is_expired(2_001));
    }

    #[test]
    fn rejects_values_that_are_not_ltpa_tokens() {
        // Given inputs that are not LTPA tokens: plain text, valid base64 of
        // the wrong shape, and an empty string.
        let wrong_shape = STANDARD.encode(b"hello world, definitely not a token");

        // When parsed.
        // Then each is rejected so the caller can fall back to the network.
        assert!(parse_ltpa_expiry("not-base64!!").is_none());
        assert!(parse_ltpa_expiry("").is_none());
        assert!(parse_ltpa_expiry(&wrong_shape).is_none());
    }

    #[test]
    fn rejects_a_token_whose_expiry_precedes_its_issue_time() {
        // Given a token whose fields are transposed or corrupt.
        let encoded = token_with(2_000, 1_000);

        // When parsed.
        // Then it is rejected rather than yielding a nonsensical window.
        assert!(parse_ltpa_expiry(&encoded).is_none());
    }

    #[test]
    fn rejects_non_hex_timestamps() {
        // Given a token with the version marker but a non-hex timestamp field.
        let mut raw = vec![0x00, 0x01, 0x02, 0x03];
        raw.extend_from_slice(b"ZZZZZZZZ");
        raw.extend_from_slice(b"6AB14DEC");
        let encoded = STANDARD.encode(raw);

        // When parsed.
        // Then it is rejected.
        assert!(parse_ltpa_expiry(&encoded).is_none());
    }
}
