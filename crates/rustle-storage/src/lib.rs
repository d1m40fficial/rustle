//! Persistent storage. M2 stores the browser session; history and bookmarks
//! move here in M3.

use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

pub const SESSION_VERSION: u32 = 1;

/// One tab as persisted between launches.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionTab {
    pub url: String,
    #[serde(default)]
    pub title: String,
}

/// Browser session file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub version: u32,
    #[serde(default)]
    pub clean_exit: bool,
    #[serde(default)]
    pub active: usize,
    #[serde(default)]
    pub tabs: Vec<SessionTab>,
    #[serde(default)]
    pub closed: Vec<SessionTab>,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            version: SESSION_VERSION,
            clean_exit: true,
            active: 0,
            tabs: Vec::new(),
            closed: Vec::new(),
        }
    }
}

/// Per-user data directory. Windows-first; other platforms fall back to the
/// current directory until M5 packaging adds proper XDG support.
pub fn data_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Rustle")
}

pub fn session_path() -> PathBuf {
    data_dir().join("session.json")
}

/// Load the session if it exists and has a supported version.
pub fn load_session() -> Option<Session> {
    let path = session_path();
    let bytes = fs::read(path).ok()?;
    let session: Session = serde_json::from_slice(&bytes).ok()?;
    (session.version == SESSION_VERSION).then_some(session)
}

/// Save the session. The file is small, so a direct write is acceptable in M2;
/// atomic replace will come with the storage hardening pass.
pub fn save_session(session: &Session) -> std::io::Result<()> {
    let path = session_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let bytes = serde_json::to_vec_pretty(session).map_err(std::io::Error::other)?;
    fs::write(path, bytes)
}

/// Convenience for tests and for callers that already have a path.
pub fn save_session_to(path: &Path, session: &Session) -> std::io::Result<()> {
    let bytes = serde_json::to_vec_pretty(session).map_err(std::io::Error::other)?;
    fs::write(path, bytes)
}
