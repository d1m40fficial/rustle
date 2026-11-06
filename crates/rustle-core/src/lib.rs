//! Core domain types for Rustle.
//!
//! This crate contains no UI, no engine and no storage code. It is the
//! place for tabs, navigation state, session model and commands.

pub mod address;

pub use address::{ErrorInfo, NavigationTarget, display_url, resolve_input, search_url};

use serde::{Deserialize, Serialize};

/// Stable identifier of a browser tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TabId(pub u64);

/// Application version from Cargo metadata.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
