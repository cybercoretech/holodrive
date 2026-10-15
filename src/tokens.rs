//! Device tokens: long-lived credentials for the Android app, the CLI and
//! scripts, sent as `Authorization: Bearer hd_<id>_<secret>`.
//!
//! Only a SHA-256 of each secret is stored (the secret is 256 random bits,
//! so a slow hash adds nothing), in `tokens.json` next to the config, mode
//! 0600. The file is re-read when it changes, so `holodrive token add` and
//! `revoke` work while the server runs. Browsers never attach this header by
//! themselves, so bearer requests need no CSRF token. A token can be
//! read-only, and no token can manage Samba or other tokens.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
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

#[derive(Default)]
struct Inner {
    tokens: Vec<Token>,
    seen: Option<SystemTime>,
    saved_use: u64,
}

pub struct Store {
    path: Option<PathBuf>,
    inner: Mutex<Inner>,
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
        let s = Store {
            path: Some(path),
            inner: Mutex::default(),
        };
        drop(s.lock());
        s
    }

    /// No file (tests).
    #[cfg(test)]
    pub fn memory() -> Store {
        Store {
            path: None,
            inner: Mutex::default(),
        }
    }

    /// The current tokens, re-read first if the file changed on disk.
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(path) = &self.path {
            let mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
            if mtime != inner.seen {
                inner.tokens = std::fs::read(path)
                    .ok()
                    .and_then(|b| serde_json::from_slice(&b).ok())
                    .unwrap_or_default();
                inner.seen = mtime;
            }
        }
        inner
    }

    fn save(&self, inner: &mut Inner) -> anyhow::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(&serde_json::to_vec_pretty(&inner.tokens)?)?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)?;
        inner.seen = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        Ok(())
    }

    pub fn list(&self) -> Vec<Token> {
        self.lock().tokens.clone()
    }

    /// A new token; the returned secret is shown once and never stored.
    pub fn create(&self, name: &str, read_only: bool) -> anyhow::Result<(Token, String)> {
        let name = name.trim();
        anyhow::ensure!(
            !name.is_empty() && name.chars().count() <= 64,
            "a device name is 1–64 characters"
        );
        let mut inner = self.lock();
        let id = loop {
            let id = crate::auth::random_hex(4);
            if !inner.tokens.iter().any(|t| t.id == id) {
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
        inner.tokens.push(token.clone());
        self.save(&mut inner)?;
        Ok((token, secret))
    }

    pub fn revoke(&self, id: &str) -> anyhow::Result<bool> {
        let mut inner = self.lock();
        let before = inner.tokens.len();
        inner.tokens.retain(|t| t.id != id);
        if inner.tokens.len() == before {
            return Ok(false);
        }
        self.save(&mut inner)?;
        Ok(true)
    }

    /// Check a bearer secret and note when and from where it was used.
    pub fn verify(&self, secret: &str, from: &str) -> Option<Grant> {
        let id = secret.strip_prefix("hd_")?.split('_').next()?;
        let mut inner = self.lock();
        let hash = digest(secret);
        let t = inner.tokens.iter_mut().find(|t| t.id == id)?;
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
        if at.saturating_sub(inner.saved_use) >= SAVE_USE_EVERY {
            inner.saved_use = at;
            let _ = self.save(&mut inner);
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
