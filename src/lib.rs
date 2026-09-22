//! Automatic class-activity attendance for HKMU's Online Learning Environment.
//!
//! Fetches the day's timetable from the OLE `oledb` API and submits
//! class-activity attendance over plain HTTPS. No browser or `WebDriver` is
//! involved.

pub mod adapter;
pub mod app;
pub mod config;
pub mod domain;
pub mod error;
pub mod infra;
pub mod port;
pub mod secret;
pub mod usecase;
