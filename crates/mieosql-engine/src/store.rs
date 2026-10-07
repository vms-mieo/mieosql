//! Persistence for connection profiles (metadata only — secrets live in the vault).

use std::path::PathBuf;
use std::sync::RwLock;

use chrono::Utc;

use mieosql_core::{ConnectionProfile, ErrorCode, MieoError, MieoResult, ProfileId};

#[derive(serde::Serialize, serde::Deserialize, Default)]
struct ProfilesFile {
    version: u32,
    profiles: Vec<ConnectionProfile>,
}

pub struct ProfileStore {
    path: PathBuf,
    profiles: RwLock<Vec<ConnectionProfile>>,
}

impl ProfileStore {
    pub fn load(path: PathBuf) -> MieoResult<Self> {
        let profiles = if path.exists() {
            let raw = std::fs::read_to_string(&path)?;
            let file: ProfilesFile = serde_json::from_str(&raw).map_err(|e| {
                MieoError::new(
                    ErrorCode::Data,
                    format!("cannot parse {}: {e}", path.display()),
                )
            })?;
            file.profiles
        } else {
            Vec::new()
        };
        Ok(Self {
            path,
            profiles: RwLock::new(profiles),
        })
    }

    pub fn list(&self) -> Vec<ConnectionProfile> {
        self.profiles
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn get(&self, id: &ProfileId) -> MieoResult<ConnectionProfile> {
        self.profiles
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .find(|p| &p.id == id)
            .cloned()
            .ok_or_else(|| MieoError::new(ErrorCode::NotFound, format!("profile `{id}` not found")))
    }

    /// Insert or replace a profile. Returns the stored profile.
    pub fn upsert(&self, mut profile: ConnectionProfile) -> MieoResult<ConnectionProfile> {
        let mut profiles = self.profiles.write().unwrap_or_else(|e| e.into_inner());
        let now = Utc::now();
        match profiles.iter_mut().find(|p| p.id == profile.id) {
            Some(existing) => {
                profile.created_at = existing.created_at;
                profile.updated_at = now;
                *existing = profile.clone();
            }
            None => {
                profile.created_at = now;
                profile.updated_at = now;
                profiles.push(profile.clone());
            }
        }
        self.persist(&profiles)?;
        Ok(profile)
    }

    pub fn remove(&self, id: &ProfileId) -> MieoResult<()> {
        let mut profiles = self.profiles.write().unwrap_or_else(|e| e.into_inner());
        let before = profiles.len();
        profiles.retain(|p| &p.id != id);
        if profiles.len() == before {
            return Err(MieoError::new(
                ErrorCode::NotFound,
                format!("profile `{id}` not found"),
            ));
        }
        self.persist(&profiles)
    }

    fn persist(&self, profiles: &[ConnectionProfile]) -> MieoResult<()> {
        let file = ProfilesFile {
            version: 1,
            profiles: profiles.to_vec(),
        };
        let json = serde_json::to_string_pretty(&file)?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}
