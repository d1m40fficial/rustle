//! Persistent storage: SQLite for history/bookmarks, JSON for settings and
//! the browser session.

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

pub const SESSION_VERSION: u32 = 1;
pub const SETTINGS_VERSION: u32 = 1;
pub const HISTORY_DEDUP_SECONDS: i64 = 30;

pub type StorageResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HistoryEntry {
    pub id: i64,
    pub url: String,
    pub title: String,
    pub visited_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DownloadRecord {
    pub id: i64,
    pub url: String,
    pub path: String,
    pub received: i64,
    pub total: i64,
    pub state: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BookmarkEntry {
    pub id: i64,
    pub url: String,
    pub title: String,
    pub folder: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Suggestion {
    pub url: String,
    pub title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub version: u32,
    #[serde(default = "default_search_engine")]
    pub search_engine: String,
    #[serde(default = "default_theme")]
    pub theme: String,
    #[serde(default = "default_language")]
    pub language: String,
    #[serde(default = "default_true")]
    pub particles_enabled: bool,
    #[serde(default = "default_particle_intensity")]
    pub particle_intensity: u32,
    #[serde(default = "default_startup")]
    pub startup: String,
    #[serde(default)]
    pub https_first: bool,
    #[serde(default)]
    pub clear_on_exit: bool,
    #[serde(default = "default_true")]
    pub adblock: bool,
    #[serde(default)]
    pub vertical_tabs: bool,
    #[serde(default)]
    pub sidebar_collapsed: bool,
    #[serde(default = "default_sidebar_width")]
    pub sidebar_width: u32,
    #[serde(default = "default_adblock_exceptions")]
    pub adblock_exceptions: Vec<String>,
    #[serde(default)]
    pub adblock_custom_rules: Vec<String>,
    #[serde(default = "default_containers")]
    pub containers: Vec<String>,
    #[serde(default)]
    pub default_container: Option<String>,
}

fn default_search_engine() -> String {
    "ddg".to_owned()
}

fn default_theme() -> String {
    "system".to_owned()
}

fn default_language() -> String {
    "ru".to_owned()
}

fn default_particle_intensity() -> u32 {
    100
}

fn default_containers() -> Vec<String> {
    vec!["personal".to_owned(), "work".to_owned(), "bank".to_owned()]
}

fn default_startup() -> String {
    "restore".to_owned()
}

fn default_true() -> bool {
    true
}

fn default_sidebar_width() -> u32 {
    282
}

fn default_group_hue() -> u16 {
    24
}

fn default_adblock_exceptions() -> Vec<String> {
    vec!["hltv.org".to_owned(), "vk.ru".to_owned()]
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: SETTINGS_VERSION,
            search_engine: default_search_engine(),
            theme: default_theme(),
            language: default_language(),
            particles_enabled: true,
            particle_intensity: default_particle_intensity(),
            startup: default_startup(),
            https_first: false,
            clear_on_exit: false,
            adblock: true,
            vertical_tabs: false,
            sidebar_collapsed: false,
            sidebar_width: default_sidebar_width(),
            adblock_exceptions: default_adblock_exceptions(),
            adblock_custom_rules: Vec::new(),
            containers: default_containers(),
            default_container: None,
        }
    }
}

/// One tab as persisted between launches.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionTab {
    pub url: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub group: Option<u64>,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub muted: bool,
    #[serde(default)]
    pub container: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionGroup {
    pub id: u64,
    pub name: String,
    #[serde(default)]
    pub collapsed: bool,
    #[serde(default = "default_group_hue")]
    pub hue: u16,
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
    #[serde(default)]
    pub groups: Vec<SessionGroup>,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            version: SESSION_VERSION,
            clean_exit: true,
            active: 0,
            tabs: Vec::new(),
            closed: Vec::new(),
            groups: Vec::new(),
        }
    }
}

/// Per-user data directory. Windows-first; other platforms fall back to the
/// current directory until packaging adds proper XDG support.
pub fn data_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Rustle")
}

pub fn session_path() -> PathBuf {
    data_dir().join("session.json")
}

pub fn settings_path() -> PathBuf {
    data_dir().join("settings.json")
}

pub fn db_path() -> PathBuf {
    data_dir().join("history.db")
}

/// Load the session if it exists and has a supported version.
pub fn load_session() -> Option<Session> {
    let bytes = fs::read(session_path()).ok()?;
    let session: Session = serde_json::from_slice(&bytes).ok()?;
    (session.version == SESSION_VERSION).then_some(session)
}

/// Save the session. The file is small, so a direct write is acceptable for now.
pub fn save_session(session: &Session) -> std::io::Result<()> {
    let path = session_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let bytes = serde_json::to_vec_pretty(session).map_err(std::io::Error::other)?;
    fs::write(path, bytes)
}

pub fn load_settings() -> Settings {
    let Ok(bytes) = fs::read(settings_path()) else {
        return Settings::default();
    };
    let Ok(settings) = serde_json::from_slice::<Settings>(&bytes) else {
        return Settings::default();
    };
    if settings.version == SETTINGS_VERSION {
        let mut settings = settings;
        if settings.adblock_exceptions.len() == 1 && settings.adblock_exceptions[0] == "hltv.org" {
            settings.adblock_exceptions.push("vk.ru".to_owned());
        }
        settings
    } else {
        Settings::default()
    }
}

pub fn save_settings(settings: &Settings) -> std::io::Result<()> {
    let path = settings_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let bytes = serde_json::to_vec_pretty(settings).map_err(std::io::Error::other)?;
    fs::write(path, bytes)
}

/// SQLite-backed history and bookmarks.
pub struct Storage {
    conn: Connection,
}

impl Storage {
    pub fn open_default() -> StorageResult<Self> {
        fs::create_dir_all(data_dir())?;
        let conn = Connection::open(db_path())?;
        Self::from_connection(conn)
    }

    fn from_connection(conn: Connection) -> StorageResult<Self> {
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA foreign_keys=ON;
             PRAGMA busy_timeout=5000;",
        )?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS history (
                id          INTEGER PRIMARY KEY,
                url         TEXT NOT NULL,
                title       TEXT NOT NULL DEFAULT '',
                visited_at  INTEGER NOT NULL,
                visit_count INTEGER NOT NULL DEFAULT 1
             );
             CREATE INDEX IF NOT EXISTS idx_history_visited ON history(visited_at DESC);
             CREATE INDEX IF NOT EXISTS idx_history_url ON history(url);

             CREATE TABLE IF NOT EXISTS bookmarks (
                id          INTEGER PRIMARY KEY,
                url         TEXT NOT NULL UNIQUE,
                title       TEXT NOT NULL DEFAULT '',
                folder      TEXT NOT NULL DEFAULT '',
                created_at  INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_bookmarks_created ON bookmarks(created_at DESC);

             CREATE TABLE IF NOT EXISTS downloads (
                id          INTEGER PRIMARY KEY,
                url         TEXT NOT NULL,
                path        TEXT NOT NULL,
                received    INTEGER NOT NULL DEFAULT 0,
                total       INTEGER NOT NULL DEFAULT 0,
                state       TEXT NOT NULL,
                created_at  INTEGER NOT NULL
             );

             CREATE TABLE IF NOT EXISTS meta (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
             );",
        )?;

        let _ = conn.execute(
            "ALTER TABLE bookmarks ADD COLUMN folder TEXT NOT NULL DEFAULT ''",
            [],
        );

        Ok(Self { conn })
    }

    #[cfg(test)]
    fn in_memory() -> StorageResult<Self> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    pub fn record_visit(&self, url: &str, title: &str) -> StorageResult<()> {
        let now = unix_now();
        let last: Option<(i64, i64)> = self
            .conn
            .query_row(
                "SELECT id, visited_at FROM history WHERE url = ?1 ORDER BY visited_at DESC LIMIT 1",
                params![url],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;

        if let Some((id, visited_at)) = last {
            if now - visited_at <= HISTORY_DEDUP_SECONDS {
                self.conn.execute(
                    "UPDATE history
                     SET title = ?1, visited_at = ?2, visit_count = visit_count + 1
                     WHERE id = ?3",
                    params![title, now, id],
                )?;
                return Ok(());
            }
        }

        self.conn.execute(
            "INSERT INTO history (url, title, visited_at) VALUES (?1, ?2, ?3)",
            params![url, title, now],
        )?;
        Ok(())
    }

    pub fn history(&self, query: &str, limit: u32) -> StorageResult<Vec<HistoryEntry>> {
        let limit = limit.clamp(1, 500);
        let pattern = format!("%{}%", query.trim());
        let mut statement = self.conn.prepare(
            "SELECT id, url, title, visited_at
             FROM history
             WHERE url LIKE ?1 OR title LIKE ?1
             ORDER BY visited_at DESC
             LIMIT ?2",
        )?;
        let rows = statement.query_map(params![pattern, limit], |row| {
            Ok(HistoryEntry {
                id: row.get(0)?,
                url: row.get(1)?,
                title: row.get(2)?,
                visited_at: row.get(3)?,
            })
        })?;
        let mut entries = Vec::new();
        for row in rows {
            entries.push(row?);
        }
        Ok(entries)
    }

    pub fn delete_history(&self, id: i64) -> StorageResult<()> {
        self.conn
            .execute("DELETE FROM history WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn clear_history(&self) -> StorageResult<()> {
        self.conn.execute("DELETE FROM history", [])?;
        Ok(())
    }

    pub fn save_download(&self, record: &DownloadRecord) -> StorageResult<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO downloads (id, url, path, received, total, state, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                record.id,
                record.url,
                record.path,
                record.received,
                record.total,
                record.state,
                record.created_at
            ],
        )?;
        Ok(())
    }

    pub fn update_download(
        &self,
        id: i64,
        received: i64,
        total: i64,
        state: &str,
    ) -> StorageResult<()> {
        self.conn.execute(
            "UPDATE downloads SET received = ?1, total = ?2, state = ?3 WHERE id = ?4",
            params![received, total, state, id],
        )?;
        Ok(())
    }

    pub fn downloads(&self) -> StorageResult<Vec<DownloadRecord>> {
        self.conn.execute(
            "UPDATE downloads SET state = 'interrupted' WHERE state = 'progress'",
            [],
        )?;
        let mut statement = self.conn.prepare(
            "SELECT id, url, path, received, total, state, created_at
             FROM downloads ORDER BY created_at DESC",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(DownloadRecord {
                id: row.get(0)?,
                url: row.get(1)?,
                path: row.get(2)?,
                received: row.get(3)?,
                total: row.get(4)?,
                state: row.get(5)?,
                created_at: row.get(6)?,
            })
        })?;
        let mut entries = Vec::new();
        for row in rows {
            entries.push(row?);
        }
        Ok(entries)
    }

    pub fn bookmarks(&self) -> StorageResult<Vec<BookmarkEntry>> {
        let mut statement = self
            .conn
            .prepare("SELECT id, url, title, folder, created_at FROM bookmarks ORDER BY folder, created_at DESC")?;
        let rows = statement.query_map([], |row| {
            Ok(BookmarkEntry {
                id: row.get(0)?,
                url: row.get(1)?,
                title: row.get(2)?,
                folder: row.get(3)?,
                created_at: row.get(4)?,
            })
        })?;
        let mut entries = Vec::new();
        for row in rows {
            entries.push(row?);
        }
        Ok(entries)
    }

    pub fn move_bookmark(&self, id: i64, folder: &str) -> StorageResult<()> {
        self.conn.execute(
            "UPDATE bookmarks SET folder = ?1 WHERE id = ?2",
            params![folder, id],
        )?;
        Ok(())
    }

    pub fn import_bookmarks(&self, entries: &[(String, String, String)]) -> StorageResult<()> {
        for (url, title, folder) in entries {
            self.conn.execute(
                "INSERT OR IGNORE INTO bookmarks (url, title, folder, created_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![url, title, folder, unix_now()],
            )?;
        }
        Ok(())
    }

    pub fn is_bookmarked(&self, url: &str) -> StorageResult<bool> {
        let id: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM bookmarks WHERE url = ?1",
                params![url],
                |row| row.get(0),
            )
            .optional()?;
        Ok(id.is_some())
    }

    /// Add if missing, remove if present. Returns the new bookmarked state.
    pub fn toggle_bookmark(&self, url: &str, title: &str) -> StorageResult<bool> {
        if self.is_bookmarked(url)? {
            self.conn
                .execute("DELETE FROM bookmarks WHERE url = ?1", params![url])?;
            Ok(false)
        } else {
            self.conn.execute(
                "INSERT INTO bookmarks (url, title, folder, created_at) VALUES (?1, ?2, '', ?3)",
                params![url, title, unix_now()],
            )?;
            Ok(true)
        }
    }

    pub fn remove_bookmark(&self, id: i64) -> StorageResult<()> {
        self.conn
            .execute("DELETE FROM bookmarks WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn clear_bookmarks(&self) -> StorageResult<()> {
        self.conn.execute("DELETE FROM bookmarks", [])?;
        Ok(())
    }

    pub fn suggestions(&self, query: &str, limit: u32) -> StorageResult<Vec<Suggestion>> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(Vec::new());
        }
        let pattern = format!("%{query}%");
        let limit = limit.clamp(1, 20) as usize;
        let mut scores: HashMap<String, (String, f64)> = HashMap::new();

        {
            let mut statement = self.conn.prepare(
                "SELECT url, MAX(title), COUNT(*), MAX(visited_at)
                 FROM history
                 WHERE url LIKE ?1 OR title LIKE ?1
                 GROUP BY url
                 ORDER BY COUNT(*) DESC, MAX(visited_at) DESC
                 LIMIT 16",
            )?;
            let rows = statement.query_map(params![pattern], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })?;
            for row in rows {
                let (url, title, visits) = row?;
                scores.insert(url, (title, visits as f64));
            }
        }

        {
            let mut statement = self.conn.prepare(
                "SELECT url, title FROM bookmarks
                 WHERE url LIKE ?1 OR title LIKE ?1
                 ORDER BY created_at DESC
                 LIMIT 16",
            )?;
            let rows = statement.query_map(params![pattern], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (url, title) = row?;
                let entry = scores.entry(url).or_insert((title.clone(), 0.0));
                if entry.0.is_empty() {
                    entry.0 = title;
                }
                entry.1 += 50.0;
            }
        }

        let mut items: Vec<(String, String, f64)> = scores
            .into_iter()
            .map(|(url, (title, score))| (url, title, score))
            .collect();
        items.sort_by(|a, b| {
            b.2.partial_cmp(&a.2)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });
        items.truncate(limit);

        Ok(items
            .into_iter()
            .map(|(url, title, _)| Suggestion { url, title })
            .collect())
    }
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

/// Convenience for callers that already have a path.
pub fn save_session_to(path: &Path, session: &Session) -> std::io::Result<()> {
    let bytes = serde_json::to_vec_pretty(session).map_err(std::io::Error::other)?;
    fs::write(path, bytes)
}

#[cfg(test)]
mod tests {
    use super::Storage;

    #[test]
    fn records_and_deduplicates_history() {
        let storage = Storage::in_memory().expect("storage");
        storage
            .record_visit("https://example.com", "Example")
            .unwrap();
        storage
            .record_visit("https://example.com", "Example 2")
            .unwrap();

        let history = storage.history("", 10).unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].title, "Example 2");
    }

    #[test]
    fn searches_history_and_bookmarks() {
        let storage = Storage::in_memory().expect("storage");
        storage
            .record_visit("https://example.com", "Example")
            .unwrap();
        storage
            .record_visit("https://rustle.dev", "Rustle project")
            .unwrap();
        storage
            .toggle_bookmark("https://example.com", "Example")
            .unwrap();

        let history = storage.history("rustle", 10).unwrap();
        assert_eq!(history.len(), 1);

        let suggestions = storage.suggestions("example", 10).unwrap();
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].url, "https://example.com");
    }

    #[test]
    fn toggles_bookmarks() {
        let storage = Storage::in_memory().expect("storage");
        assert!(
            storage
                .toggle_bookmark("https://example.com", "Example")
                .unwrap()
        );
        assert!(storage.is_bookmarked("https://example.com").unwrap());
        assert!(
            !storage
                .toggle_bookmark("https://example.com", "Example")
                .unwrap()
        );
        assert!(!storage.is_bookmarked("https://example.com").unwrap());
    }
}
