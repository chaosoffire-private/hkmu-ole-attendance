//! Infrastructure: the HTTP sub-layer the adapters drive.
//!
//! These modules handle `reqwest`, cookies, and the shape of the portal's HTML.
//! They sit below [`crate::adapter`], which exposes them to the application
//! through the ports — so no use case names any of it, and nothing here depends
//! on [`crate::usecase`] or [`crate::port`].

pub mod auth;
pub mod cookie;
pub mod html;
pub mod ltpa;
pub mod oleconnect;
pub mod session;
pub mod session_cache;
pub(crate) mod wire;

pub use session_cache::SessionCache;
