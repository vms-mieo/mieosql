//! Team workspace bundles (Collaboration v1): export/import connection
//! metadata as a portable JSON file. Secrets are **never** included — the
//! receiving teammate re-enters passwords, so bundles can be shared anywhere.
//!
//! Part of the v1 feature pack — frozen contract.

use std::collections::HashSet;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use mieosql_core::{ConnectionProfile, ErrorCode, MieoError};
use mieosql_engine::ProfileSecrets;

use crate::dto::ApiResult;
use crate::service::Api;

/// Bundle format version understood by this build.
const BUNDLE_VERSION: u32 = 1;

/// A shareable bundle of connection metadata.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WorkspaceBundle {
    pub version: u32,
    pub app: String,
    /// RFC 3339 timestamp.
    pub exported_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub note: Option<String>,
    /// Profiles without secrets.
    pub profiles: Vec<ConnectionProfile>,
}

/// Outcome of importing a bundle.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ImportSummary {
    #[ts(type = "number")]
    pub imported: u64,
    #[ts(type = "number")]
    pub skipped: u64,
    /// Names that collided and were skipped (or replaced with `overwrite`).
    #[serde(default)]
    pub names: Vec<String>,
}

impl Api {
    /// Write a workspace bundle (connection metadata, no secrets) to `path`.
    pub fn export_workspace(&self, path: String) -> ApiResult<WorkspaceBundle> {
        let bundle = WorkspaceBundle {
            version: BUNDLE_VERSION,
            app: "MieoSQL".to_string(),
            exported_at: Utc::now().to_rfc3339(),
            note: None,
            profiles: self.list_profiles(),
        };
        let json = serde_json::to_string_pretty(&bundle).map_err(|e| {
            MieoError::new(
                ErrorCode::Data,
                format!("cannot serialize workspace bundle: {e}"),
            )
        })?;
        std::fs::write(&path, json).map_err(|e| {
            MieoError::new(
                ErrorCode::Data,
                format!("cannot write workspace bundle `{path}`: {e}"),
            )
            .with_hint("check that the destination folder exists and is writable")
        })?;
        Ok(bundle)
    }

    /// Import profiles from a bundle. With `overwrite`, same-id profiles are
    /// replaced; otherwise collisions are skipped.
    pub fn import_workspace(&self, path: String, overwrite: bool) -> ApiResult<ImportSummary> {
        let raw = std::fs::read_to_string(&path).map_err(|e| {
            MieoError::new(
                ErrorCode::Data,
                format!("cannot read workspace bundle `{path}`: {e}"),
            )
            .with_hint("check that the file exists and is readable")
        })?;
        let bundle: WorkspaceBundle = serde_json::from_str(&raw).map_err(|e| {
            MieoError::new(
                ErrorCode::Data,
                format!("invalid workspace bundle `{path}`: {e}"),
            )
            .with_hint("expected a JSON file exported by MieoSQL (Workspace -> Export)")
        })?;
        if bundle.version > BUNDLE_VERSION {
            return Err(MieoError::new(
                ErrorCode::Data,
                format!("workspace bundle version {} is not supported", bundle.version),
            )
            .with_hint("update MieoSQL and try again")
            .into());
        }

        let mut known: HashSet<String> = self
            .list_profiles()
            .into_iter()
            .map(|profile| profile.id.0)
            .collect();
        let mut imported = 0u64;
        let mut skipped = 0u64;
        let mut names = Vec::new();

        for profile in bundle.profiles {
            let collides = known.contains(&profile.id.0);
            if collides && !overwrite {
                skipped += 1;
                names.push(profile.name);
                continue;
            }
            if collides {
                names.push(profile.name.clone());
            }
            // Secrets are never imported: `ProfileSecrets::default()` is a no-op
            // for the vault, so a previously stored password stays untouched.
            self.engine()
                .save_profile(&profile, &ProfileSecrets::default())?;
            known.insert(profile.id.0);
            imported += 1;
        }

        Ok(ImportSummary {
            imported,
            skipped,
            names,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mieosql_core::{Engine as DbEngine, ProfileId};
    use mieosql_engine::{Engine, EngineConfig};

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "mieosql-workspace-{tag}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn api_in(dir: &std::path::Path) -> Api {
        let engine = Engine::open(EngineConfig {
            data_dir: dir.to_path_buf(),
        })
        .unwrap();
        Api::new(engine)
    }

    fn sample_profile(name: &str, id: &str) -> ConnectionProfile {
        let mut profile = ConnectionProfile::new(name, DbEngine::Postgres, Utc::now());
        profile.id = ProfileId(id.to_string());
        profile
    }

    #[test]
    fn export_import_round_trip() {
        let dir = temp_dir("roundtrip");
        let path = dir.join("bundle.json");

        let source = api_in(&dir.join("data-source"));
        source
            .engine()
            .save_profile(&sample_profile("Local PG", "p_a"), &ProfileSecrets::default())
            .unwrap();

        let bundle = source
            .export_workspace(path.to_string_lossy().into_owned())
            .unwrap();
        assert_eq!(bundle.version, 1);
        assert_eq!(bundle.app, "MieoSQL");
        assert_eq!(bundle.profiles.len(), 1);
        assert!(!bundle.exported_at.is_empty());

        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(
            !raw.to_lowercase().contains("password"),
            "bundles must never contain secrets"
        );

        let target = api_in(&dir.join("data-target"));
        let summary = target
            .import_workspace(path.to_string_lossy().into_owned(), false)
            .unwrap();
        assert_eq!(summary.imported, 1);
        assert_eq!(summary.skipped, 0);
        assert!(target.list_profiles().iter().any(|p| p.name == "Local PG"));

        // Secrets never cross the bundle: the vault has no password for it.
        let spec = target
            .engine()
            .resolve_spec(&ProfileId("p_a".to_string()))
            .unwrap();
        assert!(spec.password.is_none());

        // Second import without overwrite -> collision is skipped.
        let summary = target
            .import_workspace(path.to_string_lossy().into_owned(), false)
            .unwrap();
        assert_eq!(summary.imported, 0);
        assert_eq!(summary.skipped, 1);
        assert_eq!(summary.names, vec!["Local PG".to_string()]);

        // With overwrite the collision is replaced and reported.
        let summary = target
            .import_workspace(path.to_string_lossy().into_owned(), true)
            .unwrap();
        assert_eq!(summary.imported, 1);
        assert_eq!(summary.skipped, 0);
        assert_eq!(summary.names, vec!["Local PG".to_string()]);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn import_reports_bad_files_with_a_hint() {
        let dir = temp_dir("bad");
        let path = dir.join("bundle.json");
        std::fs::write(&path, b"{ this is not json").unwrap();

        let api = api_in(&dir.join("data"));
        let err = api
            .import_workspace(path.to_string_lossy().into_owned(), false)
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::Data);
        assert!(err.hint.is_some());

        let missing = dir.join("nope.json");
        let err = api
            .import_workspace(missing.to_string_lossy().into_owned(), false)
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::Data);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn import_rejects_newer_bundle_versions() {
        let dir = temp_dir("version");
        let path = dir.join("bundle.json");
        std::fs::write(
            &path,
            br#"{"version":9,"app":"MieoSQL","exported_at":"2026-01-01T00:00:00Z","profiles":[]}"#,
        )
        .unwrap();

        let api = api_in(&dir.join("data"));
        let err = api
            .import_workspace(path.to_string_lossy().into_owned(), false)
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::Data);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
