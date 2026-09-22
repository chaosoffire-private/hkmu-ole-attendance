//! Authentication: a supplied session cookie, or the full NAM/Domino SSO chain.

use tracing::{debug, info, warn};

use super::cookie::Cookie;
use super::html;
use super::session::HttpSession;
use crate::config::{Config, Credentials};
use crate::error::{AppError, Result};

/// Cookie domain covering every host this client talks to.
const UNIVERSITY_DOMAIN: &str = ".hkmu.edu.hk";
/// Marker identifying the Domino silent-SSO bridge form.
const SILENT_SSO_MARKER: &str = "nov-ss-ff-silent";
/// The password field the NAM credential form names.
const NAM_USER_FIELD: &str = "Ecom_User_ID";
/// The password field the NAM credential form names.
const NAM_PASSWORD_FIELD: &str = "Ecom_Password";

/// Establishes an authenticated [`HttpSession`] from the configured credentials.
#[derive(Debug)]
pub struct Authenticator {
    config: Config,
}

impl Authenticator {
    /// Create an authenticator for the given configuration.
    pub const fn new(config: Config) -> Self {
        Self { config }
    }

    /// Produce an authenticated session.
    ///
    /// `SESSION_COOKIE` is used verbatim when present; otherwise the SSO chain
    /// is driven with `STUDENT_ID` / `STUDENT_PASSWORD`.
    ///
    /// # Errors
    /// Returns [`AppError::SessionRejected`] when a supplied cookie is not
    /// accepted, or [`AppError::Auth`] when the login chain fails.
    pub async fn authenticate(&self) -> Result<HttpSession> {
        match &self.config.credentials {
            Credentials::SessionCookie(secret) => {
                info!(mode = "session-cookie", "authenticating");
                self.seed_session_cookie(secret.expose()).await
            }
            Credentials::Password {
                student_id,
                password,
            } => {
                info!(mode = "password-sso", "authenticating");
                self.via_sso(student_id.expose(), password.expose()).await
            }
        }
    }

    /// Seed a session from a raw `Cookie:` header value and validate it.
    async fn seed_session_cookie(&self, raw: &str) -> Result<HttpSession> {
        let mut session = HttpSession::new()?;
        let mut count = 0_usize;

        for entry in raw.split(';') {
            let entry = entry.trim();
            if entry.is_empty() {
                continue;
            }
            let (name, value) = entry.split_once('=').ok_or_else(|| {
                AppError::Config(format!("SESSION_COOKIE entry {entry:?} is not name=value"))
            })?;
            session.cookies_mut().upsert(Cookie {
                name: name.trim().to_owned(),
                value: value.trim().to_owned(),
                domain: UNIVERSITY_DOMAIN.to_owned(),
                path: "/".to_owned(),
                secure: false,
            });
            count = count.saturating_add(1);
        }

        if count == 0 {
            return Err(AppError::Config(
                "SESSION_COOKIE contained no cookies".to_owned(),
            ));
        }
        debug!(cookies = session.cookies().len(), "seeded cookie jar");

        if self.validate(&mut session).await? {
            info!(cookies = session.cookies().len(), "session cookie accepted");
            return Ok(session);
        }
        Err(AppError::SessionRejected(
            "OLE rejected SESSION_COOKIE: refresh it from a logged-in browser".to_owned(),
        ))
    }

    /// Drive the full NAM -> Domino SSO chain with a username and password.
    async fn via_sso(&self, student_id: &str, password: &str) -> Result<HttpSession> {
        let mut session = HttpSession::new()?;

        let landing = session.get(&self.config.ole_url).await?;
        debug!(bytes = landing.len(), "fetched login landing page");

        let credentials_form = session
            .post_form(
                &self.config.nam_login_url,
                &[
                    ("option", "credential"),
                    (NAM_USER_FIELD, student_id),
                    (NAM_PASSWORD_FIELD, password),
                    ("loginButton2", "Login"),
                ],
            )
            .await?;

        let assertion_url = html::script_redirect(&credentials_form).ok_or_else(|| {
            AppError::Auth(
                "NAM did not return a post-login redirect; password may be wrong".to_owned(),
            )
        })?;
        debug!("following SAML assertion");

        let assertion = session.get(&assertion_url).await?;
        let bridge_action = html::form_action_containing(&assertion, SILENT_SSO_MARKER)
            .or_else(|| html::first_form_action(&assertion))
            .ok_or_else(|| {
                AppError::Auth("OLE landing page exposed no Domino login form".to_owned())
            })?;

        let bridge_url = absolute(&self.config.ole_url, &bridge_action)?;
        let bridge = session
            .post_form(
                &bridge_url,
                &[
                    ("%%ModDate", "0000000000000000"),
                    ("RedirectTo", "/OLEhome.nsf/OLEHome?ReadForm"),
                ],
            )
            .await?;

        let domino_user = html::input_value_for(&bridge, "Username").ok_or_else(|| {
            AppError::Auth("Domino bridge page carried no username field".to_owned())
        })?;
        let domino_password = html::input_value_for(&bridge, "Password").ok_or_else(|| {
            AppError::Auth("Domino bridge page carried no password field".to_owned())
        })?;

        let login_action = html::form_action_containing(&bridge, "names.nsf").ok_or_else(|| {
            AppError::Auth("Domino bridge page carried no login action".to_owned())
        })?;
        let login_url = absolute(&self.config.ole_url, &login_action)?;

        let dashboard = session
            .post_form(
                &login_url,
                &[
                    ("%%ModDate", "0000000000000000"),
                    ("RedirectTo", "/OLEhome.nsf/OLEhome?ReadForm"),
                    ("Username", &domino_user),
                    ("Password", &domino_password),
                ],
            )
            .await?;

        if !dashboard.contains("myOLE") && !dashboard.contains("Welcome") {
            warn!("dashboard markers absent after Domino login");
        }

        if self.validate(&mut session).await? {
            info!(cookies = session.cookies().len(), "SSO login succeeded");
            return Ok(session);
        }
        Err(AppError::Auth(
            "SSO chain completed but the OLE API still rejected the session".to_owned(),
        ))
    }

    /// Confirm the session can call the class API.
    async fn validate(&self, session: &mut HttpSession) -> Result<bool> {
        let payload: crate::domain::schedule::TodayClassResponse =
            session.get_json(&self.config.oleconnect_api_url).await?;
        if payload.is_success() {
            return Ok(true);
        }
        if let Some(summary) = payload.error_summary() {
            debug!(error = %summary, "OLE API rejected the session");
        }
        Ok(false)
    }
}

/// Resolve a possibly-relative URL against a base.
fn absolute(base: &str, reference: &str) -> Result<String> {
    Ok(url::Url::parse(base)?.join(reference)?.to_string())
}

#[cfg(test)]
mod tests {
    use super::{UNIVERSITY_DOMAIN, absolute};
    use crate::infra::cookie::Cookie;

    #[test]
    fn resolves_relative_form_actions_against_the_portal() {
        // Given the relative action the Domino bridge page declares.
        // When resolved against the portal base.
        let resolved = absolute(
            "https://iole.hkmu.edu.hk",
            "/names.nsf?nov-ss-ff-silent&mastercdnioleLogin33310&Login",
        )
        .expect("relative URL resolves");

        // Then it becomes an absolute university URL.
        assert_eq!(
            resolved,
            "https://iole.hkmu.edu.hk/names.nsf?nov-ss-ff-silent&mastercdnioleLogin33310&Login"
        );
    }

    #[test]
    fn seeds_session_cookies_for_the_whole_university() {
        // Given a cookie built the way the session-cookie path builds them.
        let cookie = Cookie {
            name: "LtpaToken".to_owned(),
            value: "v".to_owned(),
            domain: UNIVERSITY_DOMAIN.to_owned(),
            path: "/".to_owned(),
            secure: false,
        };

        // When checked against both hosts the client uses.
        // Then it is sent to each, since both live under the university domain.
        for host in [
            "https://iole.hkmu.edu.hk/x",
            "https://oleconnect.hkmu.edu.hk/y",
        ] {
            let url = url::Url::parse(host).expect("valid URL");
            assert!(cookie.matches(&url), "cookie should reach {host}");
        }
    }
}
