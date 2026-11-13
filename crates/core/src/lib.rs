//! Castoff's sync engine: the macOS Podcasts library, external drives, and copying between them.
//!
//! The UI talks to [`api::Backend`]; [`LiveBackend`] implements it for this Mac. Everything it does
//! is built from functions that take their paths explicitly, so each piece can be tested against
//! temp directories and fixture files:
//!
//! * [`library`]: reading the Podcasts SQLite database
//! * [`drives`]: finding external volumes (`diskutil` output, cached and time-bounded)
//! * [`drive_scan`]: reading a drive's `Podcasts` folder
//! * [`matching`]: which Mac episodes are already on the drive
//! * [`sync_engine`]: planning, copying with progress, tagging, cleanup, deleting
//! * [`settings`]: the `castoff.jsonc` settings file
//! * [`format`]: display formatters and filename sanitising

pub mod api;
pub mod drive_scan;
pub mod drives;
pub mod format;
pub mod fsutil;
pub mod library;
mod live;
pub mod matching;
pub mod settings;
pub mod sync_engine;
pub mod tagging;

pub use api::*;
pub use live::{LiveBackend, LiveConfig};
