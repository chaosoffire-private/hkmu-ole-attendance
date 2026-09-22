//! A minimal RFC 6265 cookie store for the hosts involved in the OLE login chain.

use url::Url;

/// A single stored cookie.
#[derive(Clone, PartialEq, Eq)]
pub struct Cookie {
    /// Cookie name.
    pub name: String,
    /// Cookie value.
    pub value: String,
    /// Domain attribute; a leading dot means "includes subdomains".
    pub domain: String,
    /// Path attribute.
    pub path: String,
    /// Whether the cookie may only be sent over HTTPS.
    pub secure: bool,
}

impl std::fmt::Debug for Cookie {
    /// Renders the value as a redacted, length-only summary.
    ///
    /// This type is reachable from `tracing` instrumentation, so printing the
    /// value would write a live session token into the logs.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cookie")
            .field("name", &self.name)
            .field(
                "value",
                &format_args!("<redacted {} chars>", self.value.len()),
            )
            .field("domain", &self.domain)
            .field("path", &self.path)
            .field("secure", &self.secure)
            .finish()
    }
}

impl Cookie {
    /// Whether this cookie should accompany a request to `url`.
    pub fn matches(&self, url: &Url) -> bool {
        let Some(host) = url.host_str() else {
            return false;
        };
        if !host_matches(host, &self.domain) {
            return false;
        }
        if self.secure && url.scheme() != "https" {
            return false;
        }
        path_matches(url.path(), &self.path)
    }
}

/// Domain-match per RFC 6265 §5.1.3, case-insensitively.
fn host_matches(host: &str, domain: &str) -> bool {
    let host = host.to_ascii_lowercase();
    let domain = domain.to_ascii_lowercase();
    domain.strip_prefix('.').map_or_else(
        || host == domain,
        |suffix| host == suffix || host.ends_with(&domain),
    )
}

/// Path-match per RFC 6265 §5.1.4, treating an empty attribute as `/`.
fn path_matches(request_path: &str, cookie_path: &str) -> bool {
    let cookie_path = if cookie_path.is_empty() {
        "/"
    } else {
        cookie_path
    };
    if request_path == cookie_path {
        return true;
    }
    if !request_path.starts_with(cookie_path) {
        return false;
    }
    cookie_path.ends_with('/') || request_path.as_bytes().get(cookie_path.len()) == Some(&b'/')
}

/// An in-memory jar that replaces any cookie with the same (domain, path, name).
#[derive(Debug, Default, Clone)]
pub struct CookieStore {
    cookies: Vec<Cookie>,
}

impl CookieStore {
    /// Insert a cookie, superseding an identical (domain, path, name) entry.
    pub fn upsert(&mut self, cookie: Cookie) {
        self.cookies.retain(|existing| {
            !(existing.name == cookie.name
                && existing.domain == cookie.domain
                && existing.path == cookie.path)
        });
        self.cookies.push(cookie);
    }

    /// Parse one `Set-Cookie` header value seen while talking to `request_url`.
    ///
    /// Returns `None` for malformed headers rather than failing the request.
    pub fn parse_set_cookie(header: &str, request_url: &Url) -> Option<Cookie> {
        let mut parts = header.split(';');
        let pair = parts.next()?.trim();
        let (name, value) = pair.split_once('=')?;
        let name = name.trim();
        if name.is_empty() {
            return None;
        }

        let default_domain = request_url.host_str().unwrap_or_default().to_owned();
        let default_path = default_path_for(request_url.path());
        let mut cookie = Cookie {
            name: name.to_owned(),
            value: value.trim().to_owned(),
            domain: default_domain,
            path: default_path,
            secure: false,
        };

        for attribute in parts {
            let attribute = attribute.trim();
            let (key, val) = match attribute.split_once('=') {
                Some((key, val)) => (key.trim(), val.trim()),
                None => (attribute, ""),
            };
            match key.to_ascii_lowercase().as_str() {
                "domain" => {
                    if !val.is_empty() {
                        val.clone_into(&mut cookie.domain);
                    }
                }
                "path" => {
                    if !val.is_empty() {
                        val.clone_into(&mut cookie.path);
                    }
                }
                "secure" => cookie.secure = true,
                _ => {}
            }
        }

        if cookie.domain.is_empty() {
            return None;
        }
        Some(cookie)
    }

    /// Build a `Cookie:` header value for `url`, or an empty string when none apply.
    pub fn header_for(&self, url: &Url) -> String {
        self.cookies
            .iter()
            .filter(|cookie| cookie.matches(url))
            .map(|cookie| format!("{}={}", cookie.name, cookie.value))
            .collect::<Vec<_>>()
            .join("; ")
    }

    /// Look up the value of the first cookie with this name.
    pub fn value_of(&self, name: &str) -> Option<&str> {
        self.cookies
            .iter()
            .find(|cookie| cookie.name == name)
            .map(|cookie| cookie.value.as_str())
    }

    /// Number of stored cookies, for diagnostics.
    pub fn len(&self) -> usize {
        self.cookies.len()
    }

    /// Whether the jar holds no cookies.
    pub fn is_empty(&self) -> bool {
        self.cookies.is_empty()
    }

    /// Ingest every `Set-Cookie` header from a response.
    pub fn ingest(&mut self, headers: &reqwest::header::HeaderMap, request_url: &Url) {
        for value in headers.get_all(reqwest::header::SET_COOKIE) {
            if let Ok(text) = value.to_str() {
                if let Some(cookie) = Self::parse_set_cookie(text, request_url) {
                    self.upsert(cookie);
                }
            }
        }
    }
}

/// Default cookie path: the request directory, per RFC 6265 §5.1.4.
fn default_path_for(request_path: &str) -> String {
    if !request_path.starts_with('/') {
        return "/".to_owned();
    }
    match request_path.rfind('/') {
        Some(0) | None => "/".to_owned(),
        Some(index) => request_path.get(..index).unwrap_or("/").to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use url::Url;

    use super::{Cookie, CookieStore, default_path_for, host_matches};

    fn url(value: &str) -> Url {
        Url::parse(value).expect("test URL is valid")
    }

    #[test]
    fn leading_dot_domain_matches_subdomains_and_apex() {
        // Given a domain cookie scoped to the university.
        // When matched against its apex and a subdomain.
        // Then both match, and an unrelated host does not.
        assert!(host_matches("iole.hkmu.edu.hk", ".hkmu.edu.hk"));
        assert!(host_matches("hkmu.edu.hk", ".hkmu.edu.hk"));
        assert!(!host_matches("evil.com", ".hkmu.edu.hk"));
    }

    #[test]
    fn bare_domain_matches_only_that_host() {
        // Given a cookie without a leading dot.
        // When matched against a subdomain.
        // Then only the exact host matches.
        assert!(host_matches("auth.hkmu.edu.hk", "auth.hkmu.edu.hk"));
        assert!(!host_matches("iole.hkmu.edu.hk", "auth.hkmu.edu.hk"));
    }

    #[test]
    fn cookie_is_not_sent_to_unrelated_hosts() {
        // Given a cookie for the university.
        let cookie = Cookie {
            name: "LtpaToken".to_owned(),
            value: "abc".to_owned(),
            domain: ".hkmu.edu.hk".to_owned(),
            path: "/".to_owned(),
            secure: false,
        };

        // When checking an external host.
        // Then the cookie is withheld, preventing credential leakage.
        assert!(!cookie.matches(&url("https://discord.com/api/webhooks/x")));
        assert!(cookie.matches(&url("https://oleconnect.hkmu.edu.hk/oledb/api/x")));
    }

    #[test]
    fn secure_cookie_is_withheld_from_plain_http() {
        // Given a Secure cookie.
        let cookie = Cookie {
            name: "JSESSIONID".to_owned(),
            value: "abc".to_owned(),
            domain: "auth.hkmu.edu.hk".to_owned(),
            path: "/nidp".to_owned(),
            secure: true,
        };

        // When checking an http:// URL.
        // Then it is withheld over plaintext.
        assert!(!cookie.matches(&url("http://auth.hkmu.edu.hk/nidp/x")));
        assert!(cookie.matches(&url("https://auth.hkmu.edu.hk/nidp/x")));
    }

    #[test]
    fn parses_domain_and_secure_attributes() {
        // Given a realistic NAM Set-Cookie header.
        let header = "JSESSIONID=ABC123; Path=/nidp; Secure; HttpOnly";

        // When parsed against the originating URL.
        let parsed =
            CookieStore::parse_set_cookie(header, &url("https://auth.hkmu.edu.hk/nidp/idff/sso"))
                .expect("header is well formed");

        // Then name, value, path and secure survive; HttpOnly is ignored.
        assert_eq!(parsed.name, "JSESSIONID");
        assert_eq!(parsed.value, "ABC123");
        assert_eq!(parsed.path, "/nidp");
        assert!(parsed.secure);
        assert_eq!(parsed.domain, "auth.hkmu.edu.hk");
    }

    #[test]
    fn parses_parent_domain_cookie() {
        // Given a cookie explicitly scoped to the whole university.
        let header = "LtpaToken=xyz; Path=/; Domain=.hkmu.edu.hk";

        // When parsed.
        let parsed = CookieStore::parse_set_cookie(header, &url("https://iole.hkmu.edu.hk/x"))
            .expect("header is well formed");

        // Then the parent domain is recorded verbatim.
        assert_eq!(parsed.domain, ".hkmu.edu.hk");
    }

    #[test]
    fn upsert_replaces_the_same_cookie() {
        // Given a jar holding one cookie.
        let mut store = CookieStore::default();
        let mut cookie = Cookie {
            name: "LtpaToken".to_owned(),
            value: "old".to_owned(),
            domain: ".hkmu.edu.hk".to_owned(),
            path: "/".to_owned(),
            secure: false,
        };
        store.upsert(cookie.clone());

        // When the same cookie is stored with a new value.
        cookie.value = "new".to_owned();
        store.upsert(cookie);

        // Then the jar holds one entry carrying the new value.
        assert_eq!(store.len(), 1);
        assert_eq!(
            store.header_for(&url("https://iole.hkmu.edu.hk/")),
            "LtpaToken=new"
        );
    }

    #[test]
    fn builds_a_header_only_from_applicable_cookies() {
        // Given a jar with a university cookie and an unrelated one.
        let mut store = CookieStore::default();
        for (name, domain) in [("LtpaToken", ".hkmu.edu.hk"), ("token", "example.com")] {
            store.upsert(Cookie {
                name: name.to_owned(),
                value: "v".to_owned(),
                domain: domain.to_owned(),
                path: "/".to_owned(),
                secure: false,
            });
        }

        // When a header is built for the university host.
        // Then only the applicable cookie is included.
        assert_eq!(
            store.header_for(&url("https://iole.hkmu.edu.hk/x")),
            "LtpaToken=v"
        );
    }

    #[test]
    fn debug_never_reveals_a_cookie_value() {
        // Given a cookie holding a live-looking session token.
        let cookie = Cookie {
            name: "LtpaToken".to_owned(),
            value: "AAECAzZBQjExOUIyNkFCMTUxRjJDTj1zMTM2ODk0Mg".to_owned(),
            domain: ".hkmu.edu.hk".to_owned(),
            path: "/".to_owned(),
            secure: false,
        };

        // When it is rendered for diagnostics, as tracing instrumentation does.
        let rendered = format!("{cookie:?}");

        // Then the token does not appear, because this value reaches the logs
        // through Debug and must never be written there.
        assert!(
            !rendered.contains("AAECAzZBQjEx"),
            "cookie value leaked into Debug: {rendered}"
        );
        // And the useful identifying fields are still present.
        assert!(rendered.contains("LtpaToken"));
        assert!(rendered.contains("redacted"));
        assert!(rendered.contains(".hkmu.edu.hk"));
    }

    #[test]
    fn debug_redaction_reports_only_the_length() {
        // Given two cookies with different secret lengths.
        let short = Cookie {
            name: "a".to_owned(),
            value: "abc".to_owned(),
            domain: "d".to_owned(),
            path: "/".to_owned(),
            secure: false,
        };
        let long = Cookie {
            value: "abcdef".to_owned(),
            ..short.clone()
        };

        // When rendered.
        let short_text = format!("{short:?}");
        let long_text = format!("{long:?}");

        // Then only the length differs, so a reader can still spot truncation
        // without seeing any part of the secret.
        assert!(short_text.contains("3 chars"));
        assert!(long_text.contains("6 chars"));
    }

    #[test]
    fn rejects_malformed_set_cookie_header() {
        // Given a header with no name=value pair.
        // When parsed.
        // Then it is ignored rather than producing a bogus cookie.
        assert!(
            CookieStore::parse_set_cookie("=; Path=/", &url("https://x.hkmu.edu.hk/")).is_none()
        );
        assert!(
            CookieStore::parse_set_cookie("justaname; Path=/", &url("https://x.hkmu.edu.hk/"))
                .is_none()
        );
    }

    #[test]
    fn derives_default_path_from_request_directory() {
        // Given request paths at different depths.
        // When the default cookie path is derived.
        // Then it follows RFC 6265 directory semantics.
        assert_eq!(default_path_for("/nidp/idff/sso"), "/nidp/idff");
        assert_eq!(default_path_for("/"), "/");
        assert_eq!(default_path_for("/login"), "/");
    }

    #[test]
    fn path_match_requires_a_segment_boundary() {
        // Given a cookie scoped to /nidp.
        let cookie = Cookie {
            name: "JSESSIONID".to_owned(),
            value: "v".to_owned(),
            domain: "auth.hkmu.edu.hk".to_owned(),
            path: "/nidp".to_owned(),
            secure: false,
        };

        // When checked against a path that merely shares the prefix.
        // Then a non-segment prefix does not match.
        assert!(cookie.matches(&url("https://auth.hkmu.edu.hk/nidp/app/login")));
        assert!(!cookie.matches(&url("https://auth.hkmu.edu.hk/nidpxyz")));
    }
}
