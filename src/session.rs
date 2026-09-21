//! An HTTP session that carries host-scoped cookies across the OLE hosts.

use std::time::Duration;

use reqwest::{Client, Method, Response, redirect};
use serde::de::DeserializeOwned;
use tracing::{debug, warn};
use url::Url;

use crate::cookie::CookieStore;
use crate::error::{AppError, Result};

/// A browser-like User-Agent; the OLE front ends reject some default clients.
pub const USER_AGENT: &str = "Mozilla/5.0 (Linux; Android 16; Pixel 10) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/153.0.0.0 Mobile Safari/537.36";

const MAX_REDIRECTS: usize = 12;
const TIMEOUT_SECS: u64 = 30;

/// Stateful HTTP session sharing one cookie jar.
#[derive(Debug, Clone)]
pub struct Session {
    client: Client,
    cookies: CookieStore,
}

impl Session {
    /// Build a session with an empty jar.
    ///
    /// # Errors
    /// Returns [`AppError::Http`] if the TLS/HTTP client cannot be constructed.
    pub fn new() -> Result<Self> {
        Self::with_cookies(CookieStore::default())
    }

    /// Build a session around an existing jar.
    ///
    /// # Errors
    /// Returns [`AppError::Http`] if the TLS/HTTP client cannot be constructed.
    pub fn with_cookies(cookies: CookieStore) -> Result<Self> {
        let client = Client::builder()
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(TIMEOUT_SECS))
            .redirect(redirect::Policy::none())
            .https_only(true)
            .pool_max_idle_per_host(8)
            .build()
            .map_err(AppError::Http)?;
        Ok(Self { client, cookies })
    }

    /// The cookie jar, for inspection or seeding.
    pub const fn cookies(&self) -> &CookieStore {
        &self.cookies
    }

    /// Mutable access to the cookie jar.
    pub const fn cookies_mut(&mut self) -> &mut CookieStore {
        &mut self.cookies
    }

    /// Look up one stored cookie's value by name.
    ///
    /// Used to read the expiry embedded in `LtpaToken` without a round trip.
    pub fn cookie_value(&self, name: &str) -> Option<&str> {
        self.cookies.value_of(name)
    }

    /// Perform a GET, following redirects and harvesting cookies at every hop.
    ///
    /// # Errors
    /// Returns an error when the request fails, a location header is unusable,
    /// or the redirect chain exceeds [`MAX_REDIRECTS`].
    pub async fn get(&mut self, url: &str) -> Result<String> {
        self.send_following(Method::GET, url, None, false).await
    }

    /// Perform a GET with the XHR marker the `oledb` API requires.
    ///
    /// # Errors
    /// As [`Session::get`].
    pub async fn get_xhr(&mut self, url: &str) -> Result<String> {
        self.send_following(Method::GET, url, None, true).await
    }

    /// Perform a form POST, following redirects.
    ///
    /// # Errors
    /// As [`Session::get`].
    pub async fn post_form(&mut self, url: &str, form: &[(&str, &str)]) -> Result<String> {
        self.send_following(Method::POST, url, Some(form.to_vec()), false)
            .await
    }

    /// GET a URL and decode the body as JSON.
    ///
    /// # Errors
    /// As [`Session::get`], plus [`AppError::Json`] when the body is not JSON.
    pub async fn get_json<T: DeserializeOwned>(&mut self, url: &str) -> Result<T> {
        let body = self.get_xhr(url).await?;
        serde_json::from_str(&body).map_err(AppError::Json)
    }

    /// Follow the redirect chain for one logical request.
    async fn send_following(
        &mut self,
        method: Method,
        url: &str,
        form: Option<Vec<(&str, &str)>>,
        xhr: bool,
    ) -> Result<String> {
        let mut next = url.to_owned();
        let mut pending_form = form;
        let mut current_method = method;

        for _ in 0..MAX_REDIRECTS {
            let parsed = Url::parse(&next)?;
            let mut request = self.client.request(current_method.clone(), parsed.clone());

            let cookie_header = self.cookies.header_for(&parsed);
            if !cookie_header.is_empty() {
                request = request.header(reqwest::header::COOKIE, cookie_header);
            }
            if xhr {
                request = request.header("X-Requested-With", "XMLHttpRequest");
            }
            if let Some(fields) = pending_form.take() {
                request = request.form(&fields);
            }

            let response = request.send().await.map_err(AppError::Http)?;
            let status = response.status();
            self.cookies.ingest(response.headers(), &parsed);

            if status.is_redirection() {
                next = redirect_target(&parsed, &response)?;
                current_method = Method::GET;
                debug!(%status, target = %next, "following redirect");
                continue;
            }

            let body = response.text().await.map_err(AppError::Http)?;
            if !status.is_success() {
                warn!(%status, url = %parsed, bytes = body.len(), "non-success response");
            }
            return Ok(body);
        }

        Err(AppError::Auth(format!(
            "redirect chain from {url} exceeded {MAX_REDIRECTS} hops"
        )))
    }
}

/// Resolve the `Location` header of a redirect response.
fn redirect_target(base: &Url, response: &Response) -> Result<String> {
    let location = response
        .headers()
        .get(reqwest::header::LOCATION)
        .ok_or_else(|| AppError::Auth(format!("redirect from {base} carried no Location")))?
        .to_str()
        .map_err(|_| AppError::Auth("Location header was not valid UTF-8".to_owned()))?;
    Ok(base.join(location)?.to_string())
}

#[cfg(test)]
mod tests {
    use super::Session;
    use crate::cookie::Cookie;

    #[tokio::test]
    async fn cookies_are_scoped_to_the_university_domain() {
        // Given a session seeded with a university session cookie.
        let mut session = Session::new().expect("client builds");
        session.cookies_mut().upsert(Cookie {
            name: "LtpaToken".to_owned(),
            value: "secret".to_owned(),
            domain: ".hkmu.edu.hk".to_owned(),
            path: "/".to_owned(),
            secure: false,
        });

        // When a header is requested for an unrelated third-party host.
        let external = url::Url::parse("https://discord.com/api/webhooks/1").expect("valid URL");
        let internal =
            url::Url::parse("https://oleconnect.hkmu.edu.hk/oledb/api/x").expect("valid URL");

        // Then the cookie is withheld externally and supplied internally.
        assert!(session.cookies().header_for(&external).is_empty());
        assert_eq!(session.cookies().header_for(&internal), "LtpaToken=secret");
    }
}
