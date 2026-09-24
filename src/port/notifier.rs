//! The notification port.

/// How important a notice is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Notice {
    /// Normal progress.
    Info,
    /// Something deserves attention but is not fatal.
    Warning,
    /// A failure.
    Error,
}

/// Delivers a user-facing message.
///
/// Implementations decide the transport; the use cases only decide what to say
/// and how loudly.
///
/// Delivery is deliberately infallible: notifications are advisory, and a
/// transport outage must never suppress the work being reported. An
/// implementation that cannot deliver logs the problem and returns, so the
/// return type carries no error to drop.
///
/// The future is returned as a boxed trait object rather than `impl Future` so
/// the trait stays dyn-compatible: a composite notifier holds a
/// `Vec<Box<dyn Notifier>>` and can therefore be composed with any other
/// implementation, including another composite. The box is paid once per
/// notification — a handful a day — while the polling paths that call
/// `notify` stay generic over `N: Notifier` and are still resolved statically.
pub trait Notifier: Send + Sync {
    /// Deliver `message` at the given importance.
    fn notify<'a>(
        &'a self,
        level: Notice,
        message: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>>;
}

#[cfg(test)]
mod tests {
    use super::Notifier;

    /// Assert the property the port exists to guarantee: a `Notifier` can be
    /// held behind a pointer and mixed with any other implementation.
    #[test]
    fn the_trait_supports_dynamic_dispatch() {
        // Given two different notifier implementations.
        #[derive(Debug)]
        struct First;
        #[derive(Debug)]
        struct Second;

        impl Notifier for First {
            fn notify<'a>(
                &'a self,
                _level: super::Notice,
                _message: &'a str,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
                Box::pin(std::future::ready(()))
            }
        }
        impl Notifier for Second {
            fn notify<'a>(
                &'a self,
                _level: super::Notice,
                _message: &'a str,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
                Box::pin(std::future::ready(()))
            }
        }

        // When they are stored together behind one type.
        let mixed: Vec<Box<dyn Notifier>> = vec![Box::new(First), Box::new(Second)];

        // Then both are accepted, which `impl Future` would have forbidden.
        assert_eq!(mixed.len(), 2);
    }
}
