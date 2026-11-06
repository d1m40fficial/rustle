//! Core domain types for Rustle.
//!
//! This crate contains no UI, no engine and no storage code. It is the
//! place for tabs, navigation state, session model and commands.

use serde::{Deserialize, Serialize};

/// Stable identifier of a browser tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TabId(pub u64);

/// Application version from Cargo metadata.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
