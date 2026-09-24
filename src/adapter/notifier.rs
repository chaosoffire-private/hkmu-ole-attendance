//! Notification adapters: the log, a Discord webhook, and the broadcast over them.
//!
//! Each transport does exactly one thing. [`LogNotifier`] only logs and
//! [`DiscordNotifier`] only posts; neither mirrors the other. Delivering to more
//! than one place is [`BroadcastNotifier`]'s job, and it is itself a
//! [`Notifier`], so a broadcast can hold any other notifier — including another
//! broadcast.

use std::future::Future;
use std::pin::Pin;
use std::task::Poll;

use tracing::{error, info, warn};

use crate::port::{Notice, Notifier};

/// The future a notifier hands back, boxed so the trait stays dyn-compatible.
type Delivery<'a> = Pin<Box<dyn Future<Output = ()> + Send + 'a>>;

/// Writes every notice to the log.
#[derive(Debug, Default, Clone, Copy)]
pub struct LogNotifier;

impl Notifier for LogNotifier {
    fn notify<'a>(&'a self, level: Notice, message: &'a str) -> Delivery<'a> {
        log_notice(level, message);
        Box::pin(std::future::ready(()))
    }
}

/// Posts notices to a Discord webhook.
///
/// It holds a webhook that is known to be present: whether Discord is a
/// destination at all is decided when the notifier set is assembled, not here.
#[derive(Debug)]
pub struct DiscordNotifier {
    webhook: String,
}

impl DiscordNotifier {
    /// Build a notifier for the given webhook URL.
    pub const fn new(webhook: String) -> Self {
        Self { webhook }
    }
}

impl Notifier for DiscordNotifier {
    fn notify<'a>(&'a self, _level: Notice, message: &'a str) -> Delivery<'a> {
        Box::pin(async move {
            if let Err(error) = post_to_webhook(&self.webhook, message).await {
                // Delivery is infallible by contract, so the failure is reported
                // and swallowed. This is the transport reporting its own health,
                // not mirroring the notice into the log — that is `LogNotifier`'s
                // job, and it is only present if it was wired in.
                error!(%error, "Discord delivery failed");
            }
        })
    }
}

/// Holds any number of notifiers and delivers every notice to each of them.
///
/// It is itself a [`Notifier`], so whether a destination is one transport or a
/// whole group is invisible to the caller — a broadcast may hold another
/// broadcast. Which transports exist is a wiring decision: logging is not
/// assumed, and Discord is simply absent when no webhook is configured.
#[derive(Default)]
pub struct BroadcastNotifier {
    destinations: Vec<Box<dyn Notifier>>,
}

impl BroadcastNotifier {
    /// Build a notifier that holds the given destinations.
    pub fn new(destinations: Vec<Box<dyn Notifier>>) -> Self {
        Self { destinations }
    }

    /// Deliver to every destination at once, returning when all have finished.
    ///
    /// The destinations make progress concurrently, so the order in which they
    /// complete is not guaranteed. Use this when no destination must precede
    /// another; use [`BroadcastNotifier::notify_in_order`] when one must.
    pub async fn notify_all(&self, level: Notice, message: &str) {
        let mut pending: Vec<Delivery<'_>> = self
            .destinations
            .iter()
            .map(|destination| destination.notify(level, message))
            .collect();

        // Drive every delivery to completion together. A finished delivery is
        // dropped as it completes, so none is polled after it returned.
        std::future::poll_fn(|context| {
            pending.retain_mut(|delivery| delivery.as_mut().poll(context).is_pending());
            if pending.is_empty() {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await;
    }

    /// Deliver to every destination in the order they were given.
    ///
    /// Use this when a later destination must not begin before an earlier one
    /// has finished.
    pub async fn notify_in_order(&self, level: Notice, message: &str) {
        for destination in &self.destinations {
            destination.notify(level, message).await;
        }
    }
}

impl Notifier for BroadcastNotifier {
    fn notify<'a>(&'a self, level: Notice, message: &'a str) -> Delivery<'a> {
        // The port has one entry point, and destinations held here are
        // independent, so it takes the concurrent path.
        Box::pin(self.notify_all(level, message))
    }
}

impl std::fmt::Debug for BroadcastNotifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `dyn Notifier` is not `Debug`, so the count is reported instead.
        f.debug_struct("BroadcastNotifier")
            .field("destinations", &self.destinations.len())
            .finish()
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
        // The webhook URL *is* the credential, and reqwest embeds the request
        // URL in its error `Display`; `without_url` strips it so a delivery
        // failure cannot write the token into the log.
        .map_err(|error| error.without_url().to_string())?;

    let status = response.status();
    if status.is_success() {
        return Ok(());
    }
    let body = response.text().await.unwrap_or_default();
    Err(format!("webhook returned {status}: {body}"))
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use super::{BroadcastNotifier, LogNotifier, Notifier};
    use crate::port::Notice;

    /// A transport that records what it received, suspending once first so a
    /// concurrent delivery is genuinely interleaved rather than trivially
    /// sequential.
    #[derive(Debug, Clone)]
    struct Recording {
        label: &'static str,
        seen: Arc<Mutex<Vec<String>>>,
    }

    impl Recording {
        fn new(label: &'static str, seen: &Arc<Mutex<Vec<String>>>) -> Self {
            Self {
                label,
                seen: Arc::clone(seen),
            }
        }
    }

    impl Notifier for Recording {
        fn notify<'a>(&'a self, _level: Notice, message: &'a str) -> super::Delivery<'a> {
            Box::pin(async move {
                tokio::time::sleep(Duration::from_millis(1)).await;
                self.seen
                    .lock()
                    .expect("record lock")
                    .push(format!("{}:{message}", self.label));
            })
        }
    }

    fn entries(seen: &Arc<Mutex<Vec<String>>>) -> Vec<String> {
        seen.lock().expect("record lock").clone()
    }

    #[tokio::test]
    async fn notify_all_reaches_every_destination() {
        // Given a broadcast over two transports sharing one record.
        let seen = Arc::new(Mutex::new(Vec::new()));
        let broadcast = BroadcastNotifier::new(vec![
            Box::new(Recording::new("first", &seen)),
            Box::new(Recording::new("second", &seen)),
        ]);

        // When the concurrent path delivers one notice.
        broadcast.notify_all(Notice::Info, "hello").await;

        // Then both received it. The order is deliberately not asserted, since
        // this path makes no ordering promise.
        let mut seen = entries(&seen);
        seen.sort();
        assert_eq!(seen, ["first:hello", "second:hello"]);
    }

    #[tokio::test]
    async fn notify_in_order_preserves_the_given_order() {
        // Given a broadcast over two transports, each of which suspends.
        let seen = Arc::new(Mutex::new(Vec::new()));
        let broadcast = BroadcastNotifier::new(vec![
            Box::new(Recording::new("first", &seen)),
            Box::new(Recording::new("second", &seen)),
        ]);

        // When the ordered path delivers one notice.
        broadcast.notify_in_order(Notice::Info, "hello").await;

        // Then they completed in the order they were given, which is the
        // guarantee this path exists to provide.
        assert_eq!(entries(&seen), ["first:hello", "second:hello"]);
    }

    #[tokio::test]
    async fn a_broadcast_can_hold_another_broadcast() {
        // Given a broadcast nested inside another, as composition allows.
        let seen = Arc::new(Mutex::new(Vec::new()));
        let inner: BroadcastNotifier = BroadcastNotifier::new(vec![
            Box::new(Recording::new("inner-a", &seen)),
            Box::new(Recording::new("inner-b", &seen)),
        ]);
        let outer = BroadcastNotifier::new(vec![
            Box::new(inner),
            Box::new(Recording::new("outer", &seen)),
        ]);

        // When the outer broadcast delivers.
        outer.notify_all(Notice::Info, "hello").await;

        // Then every leaf below it received the notice, so a group and a single
        // transport are interchangeable to the caller.
        let mut seen = entries(&seen);
        seen.sort();
        assert_eq!(seen, ["inner-a:hello", "inner-b:hello", "outer:hello"]);
    }

    #[tokio::test]
    async fn the_notifier_entry_point_delivers_to_all() {
        // Given a broadcast used through the port rather than the inherent API.
        let seen = Arc::new(Mutex::new(Vec::new()));
        let broadcast = BroadcastNotifier::new(vec![
            Box::new(Recording::new("first", &seen)),
            Box::new(Recording::new("second", &seen)),
        ]);

        // When it is called as a plain `Notifier`.
        Notifier::notify(&broadcast, Notice::Warning, "via port").await;

        // Then every destination still received it.
        assert_eq!(entries(&seen).len(), 2);
    }

    #[tokio::test]
    async fn a_broadcast_with_no_destinations_is_a_no_op() {
        // Given an empty composite, as an all-optional set can produce.
        let empty = BroadcastNotifier::default();

        // When a delivery is attempted.
        // Then it does nothing and, critically, does not panic.
        empty.notify_all(Notice::Error, "nobody hears this").await;

        // And a populated one still works through the same entry point, so the
        // empty case is the only thing that was silent.
        let populated = BroadcastNotifier::new(vec![Box::new(LogNotifier)]);
        populated.notify_all(Notice::Info, "heard").await;
    }
}
