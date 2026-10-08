//! App settings persisted in `<data_dir>/settings.json`.
//!
//! Currently only query-history retention; the file is forward-compatible
//! (unknown fields are ignored, missing file means defaults).

use std::path::Path;

use mieosql_core::{MieoError, MieoResult};

use crate::history::{DEFAULT_RETENTION_DAYS, MAX_RETENTION_DAYS};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct SettingsFile {
    #[serde(default = "default_retention")]
    history_retention_days: u32,
}

fn default_retention() -> u32 {
    DEFAULT_RETENTION_DAYS
}

impl Default for SettingsFile {
    fn default() -> Self {
        Self {
            history_retention_days: DEFAULT_RETENTION_DAYS,
        }
    }
}

pub fn normalize_retention(days: u32) -> u32 {
    if days == 0 {
        return 0;
    }
    days.clamp(1, MAX_RETENTION_DAYS)
}

pub fn load_history_retention(data_dir: &Path) -> u32 {
    let path = data_dir.join("settings.json");
    if let Ok(raw) = std::fs::read_to_string(&path)
        && let Ok(file) = serde_json::from_str::<SettingsFile>(&raw)
    {
        return normalize_retention(file.history_retention_days);
    }
    DEFAULT_RETENTION_DAYS
}

pub fn save_history_retention(data_dir: &Path, days: u32) -> MieoResult<u32> {
    let days = normalize_retention(days);
    let path = data_dir.join("settings.json");
    // Preserve unknown future fields by merging with the existing file.
    let mut value = if let Ok(raw) = std::fs::read_to_string(&path)
        && let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw)
    {
        v
    } else {
        serde_json::json!({})
    };
    if let Some(obj) = value.as_object_mut() {
        obj.insert(
            "history_retention_days".to_string(),
            serde_json::json!(days),
        );
    }
    let json = serde_json::to_string_pretty(&value)
        .map_err(|e| MieoError::internal(format!("cannot serialize settings: {e}")))?;
    let tmp = data_dir.join("settings.json.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, &path)?;
    Ok(days)
}
