//! Query history: in-memory ring buffer persisted as JSON Lines.

use std::collections::VecDeque;
use std::io::Write;
use std::path::PathBuf;
use std::sync::RwLock;

use chrono::{DateTime, Utc};
use mieosql_core::{Engine, ErrorCode, ErrorInfo};
use ts_rs::TS;

const MAX_IN_MEMORY: usize = 2000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum HistoryStatus {
    Ok,
    Error,
    Cancelled,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, TS)]
#[ts(export)]
pub struct HistoryEntry {
    pub id: String,
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    pub engine: Engine,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub database: Option<String>,
    pub sql: String,
    pub started_at: DateTime<Utc>,
    #[ts(type = "number")]
    pub elapsed_ms: u64,
    #[ts(type = "number")]
    pub rows: u64,
    #[ts(type = "number")]
    pub rows_affected: u64,
    pub truncated: bool,
    pub status: HistoryStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorInfo>,
}

pub struct HistoryStore {
    path: PathBuf,
    entries: RwLock<VecDeque<HistoryEntry>>,
}

impl HistoryStore {
    pub fn load(path: PathBuf) -> Self {
        let mut entries: VecDeque<HistoryEntry> = VecDeque::new();
        if path.exists()
            && let Ok(raw) = std::fs::read_to_string(&path)
        {
            for line in raw.lines() {
                if let Ok(entry) = serde_json::from_str::<HistoryEntry>(line) {
                    entries.push_back(entry);
                }
            }
            while entries.len() > MAX_IN_MEMORY {
                entries.pop_front();
            }
        }
        Self {
            path,
            entries: RwLock::new(entries),
        }
    }

    pub fn record(&self, entry: HistoryEntry) -> Option<ErrorCode> {
        {
            let mut entries = self.entries.write().unwrap_or_else(|e| e.into_inner());
            entries.push_back(entry.clone());
            while entries.len() > MAX_IN_MEMORY {
                entries.pop_front();
            }
        }
        // Best-effort persistence; failures never break query execution.
        if let Ok(line) = serde_json::to_string(&entry)
            && let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)
        {
            let _ = writeln!(f, "{line}");
            return None;
        }
        Some(ErrorCode::Internal)
    }

    /// Most recent entries first.
    pub fn list(&self, limit: usize) -> Vec<HistoryEntry> {
        let entries = self.entries.read().unwrap_or_else(|e| e.into_inner());
        entries.iter().rev().take(limit).cloned().collect()
    }

    pub fn clear(&self) {
        let mut entries = self.entries.write().unwrap_or_else(|e| e.into_inner());
        entries.clear();
        let _ = std::fs::remove_file(&self.path);
    }
}
