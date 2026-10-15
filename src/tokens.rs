//! Device tokens: long-lived credentials for the Android app, the CLI and
//! scripts, sent as `Authorization: Bearer hd_<id>_<secret>`.
//!
//! Only a SHA-256 of each secret is stored (the secret is 256 random bits,
//! so a slow hash adds nothing), in `tokens.json` next to the config (see
//! `jsonfile`), so `holodrive token add` and `revoke` work while the
//! server runs. Browsers never attach this header by
//! themselves, so bearer requests need no CSRF token. A token can be
//! read-only, and no token can manage Samba or other tokens.

use crate::jsonfile::JsonFile;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// How often `last_used` reaches the disk (it's kept in memory meanwhile).
const SAVE_USE_EVERY: u64 = 60;

#[derive(Clone, Serialize, Deserialize)]
pub struct Token {
    pub id: String,
    pub name: String,
    hash: String,
    #[serde(default)]
    pub read_only: bool,
    pub created: u64,
    #[serde(default)]
    pub last_used: Option<u64>,
    #[serde(default)]
    pub last_from: Option<String>,
}

impl Token {
    pub fn public(&self) -> Value {
        json!({
            "id": self.id,
            "name": self.name,
            "read_only": self.read_only,
            "created": self.created,
            "last_used": self.last_used,
            "last_from": self.last_from,
        })
    }
}

/// Who a valid bearer token belongs to.
pub struct Grant {
    pub name: String,
    pub read_only: bool,
}

pub struct Store {
    file: JsonFile<Token>,
    saved_use: AtomicU64,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn digest(secret: &str) -> String {
    Sha256::digest(secret.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub fn path_for(config: &Path) -> PathBuf {
    config.with_file_name("tokens.json")
}

impl Store {
    pub fn open(path: PathBuf) -> Store {
        Store {
            file: JsonFile::open(path),
            saved_use: AtomicU64::new(0),
        }
    }

    /// No file (tests).
    #[cfg(test)]
    pub fn memory() -> Store {
        Store {
            file: JsonFile::memory(),
            saved_use: AtomicU64::new(0),
        }
    }

    pub fn list(&self) -> Vec<Token> {
        self.file.lock().list.clone()
    }

    /// A new token; the returned secret is shown once and never stored.
    pub fn create(&self, name: &str, read_only: bool) -> anyhow::Result<(Token, String)> {
        let name = name.trim();
        anyhow::ensure!(
            !name.is_empty() && name.chars().count() <= 64,
            "a device name is 1–64 characters"
        );
        let mut inner = self.file.lock();
        let id = loop {
            let id = crate::auth::random_hex(4);
            if !inner.list.iter().any(|t| t.id == id) {
                break id;
            }
        };
        let secret = format!("hd_{id}_{}", crate::auth::random_hex(32));
        let token = Token {
            id,
            name: name.to_string(),
            hash: digest(&secret),
            read_only,
            created: now(),
            last_used: None,
            last_from: None,
        };
        inner.list.push(token.clone());
        self.file.save(&mut inner)?;
        Ok((token, secret))
    }

    pub fn revoke(&self, id: &str) -> anyhow::Result<bool> {
        let mut inner = self.file.lock();
        let before = inner.list.len();
        inner.list.retain(|t| t.id != id);
        if inner.list.len() == before {
            return Ok(false);
        }
        self.file.save(&mut inner)?;
        Ok(true)
    }

    /// Check a bearer secret and note when and from where it was used.
    pub fn verify(&self, secret: &str, from: &str) -> Option<Grant> {
        let id = secret.strip_prefix("hd_")?.split('_').next()?;
        let mut inner = self.file.lock();
        let hash = digest(secret);
        let t = inner.list.iter_mut().find(|t| t.id == id)?;
        if !crate::auth::constant_time_eq(t.hash.as_bytes(), hash.as_bytes()) {
            return None;
        }
        let at = now();
        t.last_used = Some(at);
        t.last_from = Some(from.to_string());
        let grant = Grant {
            name: t.name.clone(),
            read_only: t.read_only,
        };
        if at.saturating_sub(self.saved_use.load(Ordering::Relaxed)) >= SAVE_USE_EVERY {
            self.saved_use.store(at, Ordering::Relaxed);
            let _ = self.file.save(&mut inner);
        }
        Some(grant)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_verify_revoke() {
        let s = Store::memory();
        let (t, secret) = s.create("Pixel", true).unwrap();
        assert!(secret.starts_with(&format!("hd_{}_", t.id)));
        let g = s.verify(&secret, "phone").unwrap();
        assert_eq!((g.name.as_str(), g.read_only), ("Pixel", true));
        assert_eq!(s.list()[0].last_from.as_deref(), Some("phone"));
        // Right id, wrong secret; and junk.
        let forged = format!("hd_{}_{}", t.id, "0".repeat(64));
        assert!(s.verify(&forged, "x").is_none());
        assert!(s.verify("nonsense", "x").is_none());
        assert!(s.revoke(&t.id).unwrap());
        assert!(s.verify(&secret, "phone").is_none());
        assert!(!s.revoke(&t.id).unwrap());
        assert!(s.create("  ", false).is_err());
    }

    #[test]
    fn file_is_private_holds_no_secret_and_is_reread() {
        let dir =
            std::env::temp_dir().join(format!("holodrive-tokens-{}", crate::auth::random_hex(4)));
        let path = dir.join("tokens.json");
        let server = Store::open(path.clone());
        // As if `holodrive token add` ran in another process.
        let cli = Store::open(path.clone());
        let (_, secret) = cli.create("laptop", false).unwrap();
        use std::os::unix::fs::PermissionsExt;
        let meta = std::fs::metadata(&path).unwrap();
        assert_eq!(meta.permissions().mode() & 0o777, 0o600);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains(&secret[10..]), "secret must not be stored");
        assert!(server.verify(&secret, "tailnet").is_some());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
