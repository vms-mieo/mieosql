//! Encrypted credential vault.
//!
//! Secrets never live in `profiles.json`. Each secret value is encrypted with
//! XChaCha20-Poly1305 under a random 32-byte master key stored in
//! `<data_dir>/master.key` (mode 0600 on Unix).
//!
//! This is the pragmatic v1 model: everything stays local, nothing is
//! plaintext on disk, and an OS keychain-backed store (P2) can replace the
//! `Vault` implementation without touching callers.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use base64::Engine as _;
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use rand::RngCore;

use mieosql_core::{ErrorCode, MieoError, MieoResult};

const B64: base64::engine::general_purpose::GeneralPurpose =
    base64::engine::general_purpose::STANDARD;
const NONCE_LEN: usize = 24;

#[derive(serde::Serialize, serde::Deserialize, Default)]
struct VaultFile {
    /// profile_id -> (secret name -> base64(nonce || ciphertext))
    profiles: HashMap<String, HashMap<String, String>>,
}

pub struct Vault {
    key: [u8; 32],
    path: PathBuf,
    store: RwLock<VaultFile>,
}

impl Vault {
    pub fn open(data_dir: &Path) -> MieoResult<Self> {
        let key = load_or_create_key(&data_dir.join("master.key"))?;
        let path = data_dir.join("secrets.json");
        let store: VaultFile = if path.exists() {
            let raw = std::fs::read_to_string(&path)?;
            serde_json::from_str(&raw).unwrap_or_default()
        } else {
            VaultFile::default()
        };
        Ok(Self {
            key,
            path,
            store: RwLock::new(store),
        })
    }

    /// Store a secret. An empty value deletes the secret.
    pub fn set(&self, profile_id: &str, name: &str, value: &str) -> MieoResult<()> {
        let mut store = self.store.write().unwrap_or_else(|e| e.into_inner());
        let entry = store.profiles.entry(profile_id.to_string()).or_default();
        if value.is_empty() {
            entry.remove(name);
            if entry.is_empty() {
                store.profiles.remove(profile_id);
            }
        } else {
            let ct = encrypt(&self.key, value.as_bytes())?;
            entry.insert(name.to_string(), ct);
        }
        self.persist(&store)
    }

    pub fn get(&self, profile_id: &str, name: &str) -> Option<String> {
        let store = self.store.read().unwrap_or_else(|e| e.into_inner());
        let raw = store.profiles.get(profile_id)?.get(name)?;
        decrypt(&self.key, raw).ok()
    }

    pub fn remove_profile(&self, profile_id: &str) -> MieoResult<()> {
        let mut store = self.store.write().unwrap_or_else(|e| e.into_inner());
        store.profiles.remove(profile_id);
        self.persist(&store)
    }

    fn persist(&self, store: &VaultFile) -> MieoResult<()> {
        let json = serde_json::to_string_pretty(store)?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

fn load_or_create_key(path: &Path) -> MieoResult<[u8; 32]> {
    if path.exists() {
        let bytes = std::fs::read(path)?;
        if bytes.len() != 32 {
            return Err(MieoError::new(
                ErrorCode::Data,
                format!(
                    "master key at {} is corrupt (expected 32 bytes)",
                    path.display()
                ),
            ));
        }
        let mut key = [0u8; 32];
        key.copy_from_slice(&bytes);
        return Ok(key);
    }

    let mut key = [0u8; 32];
    rand::rng().fill_bytes(&mut key);
    std::fs::write(path, key)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(path, perms)?;
    }
    Ok(key)
}

fn encrypt(key: &[u8; 32], plaintext: &[u8]) -> MieoResult<String> {
    let cipher = XChaCha20Poly1305::new(&Key::from(*key));
    let mut nonce_bytes = [0u8; NONCE_LEN];
    rand::rng().fill_bytes(&mut nonce_bytes);
    let nonce = XNonce::from(nonce_bytes);
    let ct = cipher
        .encrypt(&nonce, plaintext)
        .map_err(|_| MieoError::internal("failed to encrypt secret"))?;
    let mut out = Vec::with_capacity(NONCE_LEN + ct.len());
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ct);
    Ok(B64.encode(&out))
}

fn decrypt(key: &[u8; 32], encoded: &str) -> MieoResult<String> {
    let raw = B64
        .decode(encoded)
        .map_err(|e| MieoError::new(ErrorCode::Data, format!("corrupt vault entry: {e}")))?;
    if raw.len() <= NONCE_LEN {
        return Err(MieoError::new(ErrorCode::Data, "corrupt vault entry"));
    }
    let (nonce_bytes, ct) = raw.split_at(NONCE_LEN);
    let cipher = XChaCha20Poly1305::new(&Key::from(*key));
    let nonce: [u8; NONCE_LEN] = nonce_bytes
        .try_into()
        .map_err(|_| MieoError::new(ErrorCode::Data, "corrupt vault entry"))?;
    let plaintext = cipher.decrypt(&XNonce::from(nonce), ct).map_err(|_| {
        MieoError::new(
            ErrorCode::Data,
            "failed to decrypt secret (master key changed or vault corrupted)",
        )
    })?;
    String::from_utf8(plaintext)
        .map_err(|e| MieoError::new(ErrorCode::Data, format!("secret is not valid UTF-8: {e}")))
}
