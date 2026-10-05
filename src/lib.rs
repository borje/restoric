//! restoric: browse and restore a restic repository by time, anchored on a
//! folder. See PLAN.md.

pub mod app;
pub mod cache;
pub mod diff;
pub mod disk;
pub mod index;
pub mod log;
pub mod repo;
pub mod restore;
pub mod tui;
pub mod ui;
pub mod worker;
