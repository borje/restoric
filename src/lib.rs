//! restoric: browse and restore a restic repository by time, anchored on a
//! folder.

pub mod app;
pub mod cache;
pub mod config;
pub mod diff;
pub mod dirs;
pub mod disk;
pub mod index;
pub mod log;
pub mod picker;
pub mod repo;
pub mod repos;
pub mod restore;
pub mod tui;
pub mod ui;
pub mod worker;
