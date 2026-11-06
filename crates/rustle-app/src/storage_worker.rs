//! Background SQLite worker for history, bookmarks and address suggestions.

use std::sync::mpsc::{self, Sender};

use rustle_storage::Storage;
use serde_json::{Value, json};
use tao::event_loop::EventLoopProxy;

use crate::AppEvent;

pub enum StorageCommand {
    RecordVisit {
        url: String,
        title: String,
    },
    Query {
        request_id: u64,
        query: StorageQuery,
    },
}

pub enum StorageQuery {
    History { query: String, limit: u32 },
    Bookmarks,
    Suggest { query: String, limit: u32 },
    DeleteHistory { id: i64 },
    ClearHistory,
    ToggleBookmark { url: String, title: String },
    RemoveBookmark { id: i64 },
}

pub fn spawn(proxy: EventLoopProxy<AppEvent>) -> Option<Sender<StorageCommand>> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || run(proxy, receiver));
    Some(sender)
}

fn run(proxy: EventLoopProxy<AppEvent>, receiver: mpsc::Receiver<StorageCommand>) {
    let storage = match Storage::open_default() {
        Ok(storage) => Some(storage),
        Err(error) => {
            eprintln!("[storage] open failed: {error}");
            None
        }
    };

    while let Ok(command) = receiver.recv() {
        match command {
            StorageCommand::RecordVisit { url, title } => {
                if let Some(storage) = &storage {
                    if let Err(error) = storage.record_visit(&url, &title) {
                        eprintln!("[storage] record visit failed: {error}");
                    }
                }
            }
            StorageCommand::Query { request_id, query } => {
                let payload = match (&storage, query) {
                    (None, _) => json!({ "ok": false, "error": "storage unavailable" }),
                    (Some(storage), StorageQuery::History { query, limit }) => {
                        match storage.history(&query, limit) {
                            Ok(items) => json!({ "ok": true, "items": items }),
                            Err(error) => error_payload(&error),
                        }
                    }
                    (Some(storage), StorageQuery::Bookmarks) => match storage.bookmarks() {
                        Ok(items) => json!({ "ok": true, "items": items }),
                        Err(error) => error_payload(&error),
                    },
                    (Some(storage), StorageQuery::Suggest { query, limit }) => {
                        match storage.suggestions(&query, limit) {
                            Ok(items) => json!({ "ok": true, "items": items }),
                            Err(error) => error_payload(&error),
                        }
                    }
                    (Some(storage), StorageQuery::DeleteHistory { id }) => {
                        match storage.delete_history(id) {
                            Ok(()) => json!({ "ok": true }),
                            Err(error) => error_payload(&error),
                        }
                    }
                    (Some(storage), StorageQuery::ClearHistory) => match storage.clear_history() {
                        Ok(()) => json!({ "ok": true }),
                        Err(error) => error_payload(&error),
                    },
                    (Some(storage), StorageQuery::ToggleBookmark { url, title }) => {
                        match storage.toggle_bookmark(&url, &title) {
                            Ok(bookmarked) => json!({ "ok": true, "bookmarked": bookmarked }),
                            Err(error) => error_payload(&error),
                        }
                    }
                    (Some(storage), StorageQuery::RemoveBookmark { id }) => {
                        match storage.remove_bookmark(id) {
                            Ok(()) => json!({ "ok": true }),
                            Err(error) => error_payload(&error),
                        }
                    }
                };

                let _ = proxy.send_event(AppEvent::StorageResponse {
                    request_id,
                    payload,
                });
            }
        }
    }
}

fn error_payload(error: &dyn std::fmt::Display) -> Value {
    json!({ "ok": false, "error": error.to_string() })
}
