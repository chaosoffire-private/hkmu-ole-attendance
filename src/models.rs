//! Wire types, re-exported from the domain so there is exactly one definition.
//!
//! The OLE payload describes what a timetable *is*, which makes it domain
//! knowledge; the canonical types therefore live in
//! [`crate::domain::schedule`]. This module exists so the HTTP adapter can name
//! them under a transport-flavoured path, and so `ClassInfo` — an
//! adapter-facing view that carries the per-course URL — has a home.

pub use crate::domain::schedule::{ClassSession, Course, TodayClassResponse};

/// A class resolved for scheduling, as the HTTP client consumes it.
///
/// Adds the per-course activities URL, which is a transport concern rather than
/// a domain one.
#[derive(Debug, Clone)]
pub struct ClassInfo {
    /// Term identifier.
    pub termcode: String,
    /// Course code.
    pub course_code: String,
    /// Display name.
    pub class_name: String,
    /// Local start time.
    pub starts_at: jiff::civil::DateTime,
    /// Local end time; `None` when the API omitted it.
    pub ends_at: Option<jiff::civil::DateTime>,
    /// Group code.
    pub group: String,
    /// Room or venue.
    pub venue: String,
    /// Class-activities page for this course.
    pub activities_url: String,
}

impl ClassInfo {
    /// Stable one-line description used in logs.
    pub fn label(&self) -> String {
        format!("{} - {}", self.course_code, self.class_name)
    }
}
