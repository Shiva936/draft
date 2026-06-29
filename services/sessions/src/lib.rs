//! Session manager (FR-SVC, Blueprint §18.3). Tracks in-memory client sessions
//! connected to `draftd`. Minimal in v0.2.0 — sessions are lightweight handles
//! used for accounting and future cancellation support.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

#[derive(Debug, Clone)]
pub struct Session {
    pub id: u64,
    pub workspace_path: Option<String>,
}

#[derive(Default)]
pub struct SessionManager {
    next: AtomicU64,
    sessions: Mutex<HashMap<u64, Session>>,
}

impl SessionManager {
    pub fn new() -> Self {
        SessionManager::default()
    }

    pub fn open(&self, workspace_path: Option<String>) -> u64 {
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        self.sessions
            .lock()
            .unwrap()
            .insert(id, Session { id, workspace_path });
        id
    }

    pub fn close(&self, id: u64) {
        self.sessions.lock().unwrap().remove(&id);
    }

    pub fn count(&self) -> usize {
        self.sessions.lock().unwrap().len()
    }
}
