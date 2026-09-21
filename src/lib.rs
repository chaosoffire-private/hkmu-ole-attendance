//! Automatic class-activity attendance for HKMU's Online Learning Environment.
//!
//! Fetches the day's timetable from the OLE `oledb` API and submits
//! class-activity attendance over plain HTTPS. No browser or `WebDriver` is
//! involved.

pub mod adapter;
pub mod auth;
pub mod config;
pub mod cookie;
pub mod domain;
pub mod error;
pub mod html;
pub mod ltpa;
pub mod models;
pub mod oleconnect;
pub mod port;
pub mod scheduler;
pub mod secret;
pub mod session;
pub mod session_cache;
pub mod usecase;
