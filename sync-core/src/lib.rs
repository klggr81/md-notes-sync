//! Sync logic of md-notes-sync: the files in the app folder (sync.conf,
//! rclone.conf, filters.txt, bisync-state, sync.log, backups), one sync run
//! with rclone bisync, and the setup (rclone, sign-in, folders).
//!
//! No Tauri dependency: other apps can use this crate as it is.

pub mod backup;
pub mod conf;
pub mod history;
pub mod lock;
pub mod logline;
pub mod runner;
pub mod setup;

pub use runner::{run, Event, Options, Summary, SyncError};
