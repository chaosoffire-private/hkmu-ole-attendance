//! Notification adapters: the log, and a Discord webhook.

use tracing::{error, info, warn};

use crate::port::{Notice, Notifier, PortError};

/// Writes every notice to the log.
///
/// Always successful: logging is the fallback transport and must never be the
/// reason a caller fails.
#[derive(Debug, Default, Clone, Copy)]
pub struct LogNotifier;

impl Notifier for LogNotifier {
    async fn notify(&self, level: Notice, message: &str) -> Result<(), PortError> {
        match level {
            Notice::Info => info!("{message}"),
            Notice::Warning => warn!("{message}"),
            Notice::Error => error!("{message}"),
        }
        Ok(())
    }
}

/// Posts notices to a Discord webhook, falling back to the log when unset.
#[derive(Debug)]
pub struct DiscordNotifier {
    webhook: Option<String>,
}

impl DiscordNotifier {
    /// Build a notifier for the given webhook, if any.
    pub const fn new(webhook: Option<String>) -> Self {
        Self { webhook }
    }

    /// Whether a webhook is configured, so mirroring can actually deliver.
    pub const fn is_enabled(&self) -> bool {
        self.webhook.is_some()
    }
}

impl Notifier for DiscordNotifier {
    /// Always logged; posted when a webhook is configured.
    ///
    /// A delivery failure is logged and swallowed. Callers report *before*
    /// scheduling attendance, so propagating here would let a notification
    /// outage suppress attendance entirely.
    async fn notify(&self, level: Notice, message: &str) -> Result<(), PortError> {
        match level {
            Notice::Info => info!("{message}"),
            Notice::Warning => warn!("{message}"),
            Notice::Error => error!("{message}"),
        }

        let Some(webhook) = self.webhook.as_deref() else {
            warn!(target: "notify", "{message}");
            return Ok(());
        };

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .https_only(true)
            .build()
            .map_err(|error| PortError::Transport(error.to_string()))?;

        let response = client
            .post(webhook)
            .json(&serde_json::json!({ "content": message }))
            .send()
            .await
            .map_err(|error| PortError::Transport(error.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            error!(%status, body = %body, "Discord webhook rejected the message");
            return Ok(());
        }
        Ok(())
    }
}

/// Delivers to a primary notifier, tolerating its failure.
#[derive(Debug)]
pub struct CompositeNotifier<P, F> {
    primary: P,
    fallback: F,
}

impl<P, F> CompositeNotifier<P, F> {
    /// Pair a primary notifier with a fallback used when it fails.
    pub const fn new(primary: P, fallback: F) -> Self {
        Self { primary, fallback }
    }
}

impl<P, F> Notifier for CompositeNotifier<P, F>
where
    P: Notifier,
    F: Notifier,
{
    async fn notify(&self, level: Notice, message: &str) -> Result<(), PortError> {
        match self.primary.notify(level, message).await {
            Ok(()) => Ok(()),
            Err(error) => {
                warn!(%error, "primary notifier failed; using the fallback");
                self.fallback.notify(level, message).await
            }
        }
    }
}
