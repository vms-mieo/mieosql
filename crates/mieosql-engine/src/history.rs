//! Query history: in-memory ring buffer persisted as JSON Lines.
//!
//! Entries older than `retention_days` (default 15, `0` = keep forever) are
//! pruned automatically on load, on record and when the setting changes —
//! both in memory and in `history.jsonl` (file is compacted on prune).

use std::collections::VecDeque;
use std::io::Write;
use std::path::PathBuf;
use std::sync::RwLock;

use chrono::{DateTime, Utc};
use mieosql_core::{Engine, ErrorCode, ErrorInfo};
use ts_rs::TS;

const MAX_IN_MEMORY: usize = 2000;

/// Default number of days query history is kept.
pub const DEFAULT_RETENTION_DAYS: u32 = 15;
/// Maximum configurable retention (`0` = unlimited).
pub const MAX_RETENTION_DAYS: u32 = 3650;

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
    retention_days: RwLock<u32>,
}

fn cutoff(retention_days: u32) -> Option<DateTime<Utc>> {
    if retention_days == 0 {
        return None;
    }
    let days = retention_days.min(MAX_RETENTION_DAYS) as i64;
    Utc::now()
        .checked_sub_signed(chrono::Duration::days(days))
}

fn is_expired(started_at: &DateTime<Utc>, cutoff: &Option<DateTime<Utc>>) -> bool {
    cutoff.is_some_and(|c| *started_at < c)
}

impl HistoryStore {
    pub fn load(path: PathBuf, retention_days: u32) -> Self {
        let retention_days = normalize_retention(retention_days);
        let cutoff = cutoff(retention_days);
        let mut entries: VecDeque<HistoryEntry> = VecDeque::new();
        if path.exists()
            && let Ok(raw) = std::fs::read_to_string(&path)
        {
            for line in raw.lines() {
                if let Ok(entry) = serde_json::from_str::<HistoryEntry>(line) {
                    if is_expired(&entry.started_at, &cutoff) {
                        continue;
                    }
                    entries.push_back(entry);
                }
            }
            while entries.len() > MAX_IN_MEMORY {
                entries.pop_front();
            }
        }
        let store = Self {
            path,
            entries: RwLock::new(entries),
            retention_days: RwLock::new(retention_days),
        };
        // Compact the file on startup when expired lines were skipped.
        store.compact_file();
        store
    }

    pub fn retention_days(&self) -> u32 {
        *self.retention_days.read().unwrap_or_else(|e| e.into_inner())
    }

    /// Update retention and immediately prune memory + file. Returns normalized value.
    pub fn set_retention_days(&self, days: u32) -> u32 {
        let days = normalize_retention(days);
        *self.retention_days.write().unwrap_or_else(|e| e.into_inner()) = days;
        self.prune_expired();
        days
    }

    pub fn record(&self, entry: HistoryEntry) -> Option<ErrorCode> {
        let pruned_any = {
            let mut entries = self.entries.write().unwrap_or_else(|e| e.into_inner());
            entries.push_back(entry.clone());
            while entries.len() > MAX_IN_MEMORY {
                entries.pop_front();
            }
            prune_queue(&mut entries, &cutoff(self.retention_days()))
        };
        // Best-effort persistence; failures never break query execution.
        if let Ok(line) = serde_json::to_string(&entry)
            && let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)
        {
            let _ = writeln!(f, "{line}");
            if pruned_any {
                // File still holds the expired lines just pruned from memory —
                // compact it now so auto-delete actually frees disk.
                self.compact_file();
            }
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

    fn prune_expired(&self) {
        let pruned = {
            let mut entries = self.entries.write().unwrap_or_else(|e| e.into_inner());
            prune_queue(&mut entries, &cutoff(self.retention_days()))
        };
        if pruned {
            self.compact_file();
        }
    }

    /// Rewrite `history.jsonl` from the in-memory (already pruned) entries.
    fn compact_file(&self) {
        let entries = self.entries.read().unwrap_or_else(|e| e.into_inner());
        if let Ok(tmp) = self
            .path
            .parent()
            .map(|d| d.join("history.jsonl.tmp"))
            .ok_or(())
        {
            let mut out = String::new();
            for e in entries.iter() {
                if let Ok(line) = serde_json::to_string(e) {
                    out.push_str(&line);
                    out.push('\n');
                }
            }
            if std::fs::write(&tmp, out).is_ok() {
                let _ = std::fs::rename(&tmp, &self.path);
            }
        }
    }
}

fn prune_queue(queue: &mut VecDeque<HistoryEntry>, cutoff: &Option<DateTime<Utc>>) -> bool {
    let Some(c) = cutoff else { return false };
    let before = queue.len();
    queue.retain(|e| e.started_at >= *c);
    queue.len() != before
}

fn normalize_retention(days: u32) -> u32 {
    if days == 0 {
        return 0;
    }
    days.clamp(1, MAX_RETENTION_DAYS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(days_ago: i64) -> HistoryEntry {
        HistoryEntry {
            id: format!("h_{days_ago}"),
            session_id: "s_1".to_string(),
            profile_id: None,
            engine: Engine::Postgres,
            database: None,
            sql: "SELECT 1".to_string(),
            started_at: Utc::now() - chrono::Duration::days(days_ago),
            elapsed_ms: 1,
            rows: 1,
            rows_affected: 0,
            truncated: false,
            status: HistoryStatus::Ok,
            error: None,
        }
    }

    #[test]
    fn prunes_entries_older_than_retention() {
        let dir = std::env::temp_dir().join(format!(
            "mieosql-history-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("history.jsonl");
        let store = HistoryStore::load(path.clone(), 15);
        store.record(entry(16));
        store.record(entry(10));
        store.record(entry(0));
        assert_eq!(store.list(10).len(), 2);
        // Reload from disk: the 16-day-old entry must be gone from the file too.
        let reloaded = HistoryStore::load(path, 15);
        assert_eq!(reloaded.list(10).len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn zero_means_unlimited() {
        let dir = std::env::temp_dir().join(format!(
            "mieosql-history-unlimited-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let store = HistoryStore::load(dir.join("history.jsonl"), 0);
        store.record(entry(400));
        assert_eq!(store.list(10).len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
