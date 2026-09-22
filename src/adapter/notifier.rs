//! Notification adapters: the log, and a Discord webhook.

use tracing::{error, info, warn};

use crate::port::{Notice, Notifier};

/// Writes every notice to the log.
#[derive(Debug, Default, Clone, Copy)]
pub struct LogNotifier;

impl Notifier for LogNotifier {
    fn notify(&self, level: Notice, message: &str) -> impl std::future::Future<Output = ()> + Send {
        log_notice(level, message);
        std::future::ready(())
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
    async fn notify(&self, level: Notice, message: &str) {
        log_notice(level, message);

        let Some(webhook) = self.webhook.as_deref() else {
            return;
        };
        if let Err(error) = post_to_webhook(webhook, message).await {
            error!(%error, "Discord delivery failed; the notice was logged above");
        }
    }
}

fn log_notice(level: Notice, message: &str) {
    match level {
        Notice::Info => info!("{message}"),
        Notice::Warning => warn!("{message}"),
        Notice::Error => error!("{message}"),
    }
}

async fn post_to_webhook(webhook: &str, message: &str) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .https_only(true)
        .build()
        .map_err(|error| error.to_string())?;

    let response = client
        .post(webhook)
        .json(&serde_json::json!({ "content": message }))
        .send()
        .await
        .map_err(|error| error.to_string())?;

    let status = response.status();
    if status.is_success() {
        return Ok(());
    }
    let body = response.text().await.unwrap_or_default();
    Err(format!("webhook returned {status}: {body}"))
}
