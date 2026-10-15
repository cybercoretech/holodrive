//! Share links: `/s/<key>` gives someone one file or folder (download), or a
//! drop box (upload only, nothing listed), without the Holodrive password.
//!
//! A key is 128 random bits; links live in `links.json` next to the config
//! (see `jsonfile`) so the owner can copy them again later. A link can
//! expire, need a password (argon2id, with its own brake), and allow only
//! so many downloads. Links are reachable wherever Holodrive is (the
//! tailnet); nothing here opens it to the internet.

use crate::config::Config;
use crate::fsops::{self, FsError, FsResult};
use crate::jsonfile::JsonFile;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const MAX_FAILS: usize = 5;
const FAIL_WINDOW: Duration = Duration::from_secs(600);
/// How long a correct password keeps a link open in that browser.
pub const GRANT_SECS: u64 = 12 * 3600;

#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Look and download.
    View,
    /// Upload only, into a folder; its contents stay hidden.
    Drop,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Link {
    pub key: String,
    pub path: String,
    pub dir: bool,
    pub mode: Mode,
    #[serde(default)]
    pub label: Option<String>,
    pub created: u64,
    #[serde(default)]
    pub expires: Option<u64>,
    #[serde(default)]
    password: Option<String>,
    #[serde(default)]
    pub max_downloads: Option<u64>,
    #[serde(default)]
    pub downloads: u64,
    #[serde(default)]
    pub uploads: u64,
}

impl Link {
    pub fn has_password(&self) -> bool {
        self.password.is_some()
    }
    pub fn expired(&self) -> bool {
        self.expires.is_some_and(|t| now() >= t)
    }
    pub fn used_up(&self) -> bool {
        self.max_downloads.is_some_and(|m| self.downloads >= m)
    }
    pub fn name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }
    /// What the owner sees.
    pub fn public(&self) -> Value {
        json!({
            "key": self.key,
            "path": self.path,
            "name": self.name(),
            "dir": self.dir,
            "mode": self.mode,
            "label": self.label,
            "created": self.created,
            "expires": self.expires,
            "expired": self.expired(),
            "password": self.has_password(),
            "max_downloads": self.max_downloads,
            "downloads": self.downloads,
            "uploads": self.uploads,
        })
    }
    /// The virtual path of `sub` inside the shared item.
    pub fn target(&self, sub: &str) -> FsResult<String> {
        let sub = sub.trim_matches('/');
        if sub.is_empty() {
            Ok(self.path.clone())
        } else if self.dir {
            Ok(format!("{}/{sub}", self.path))
        } else {
            Err(FsError(400, "this link is a single file".into()))
        }
    }
}

#[derive(Deserialize)]
pub struct NewLink {
    pub path: String,
    #[serde(default = "view")]
    pub mode: Mode,
    /// Days until it expires; none or 0 = never.
    #[serde(default)]
    pub days: Option<u64>,
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub max_downloads: Option<u64>,
    #[serde(default)]
    pub label: Option<String>,
}

fn view() -> Mode {
    Mode::View
}

pub enum Unlock {
    Granted(String),
    Wrong,
    Wait(Duration),
}

pub struct Store {
    file: JsonFile<Link>,
    fails: Mutex<HashMap<String, VecDeque<Instant>>>,
    /// grant id → (link key, valid until)
    grants: Mutex<HashMap<String, (String, Instant)>>,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn path_for(config: &Path) -> PathBuf {
    config.with_file_name("links.json")
}

pub fn valid_key(key: &str) -> bool {
    key.len() == 32 && key.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

impl Store {
    fn with(file: JsonFile<Link>) -> Store {
        Store {
            file,
            fails: Mutex::default(),
            grants: Mutex::default(),
        }
    }

    pub fn open(path: PathBuf) -> Store {
        Store::with(JsonFile::open(path))
    }

    #[cfg(test)]
    pub fn memory() -> Store {
        Store::with(JsonFile::memory())
    }

    pub fn list(&self) -> Vec<Link> {
        self.file.lock().list.clone()
    }

    pub fn get(&self, key: &str) -> Option<Link> {
        if !valid_key(key) {
            return None;
        }
        self.file.lock().list.iter().find(|l| l.key == key).cloned()
    }

    pub fn create(&self, cfg: &Config, n: NewLink) -> FsResult<Link> {
        let r = fsops::resolve(cfg, &n.path)?;
        let meta = std::fs::metadata(&r.abs)?;
        if n.mode == Mode::Drop {
            if !meta.is_dir() {
                return Err(FsError(400, "a drop box has to be a folder".into()));
            }
            r.writable()?;
        }
        let label = n
            .label
            .map(|l| l.trim().chars().take(80).collect::<String>())
            .filter(|l| !l.is_empty());
        let password = match n.password.as_deref().map(str::trim) {
            Some(p) if !p.is_empty() => {
                Some(crate::auth::hash_password(p).map_err(|e| FsError(500, e.to_string()))?)
            }
            _ => None,
        };
        let link = Link {
            key: crate::auth::random_hex(16),
            path: r.vpath(),
            dir: meta.is_dir(),
            mode: n.mode,
            label,
            created: now(),
            expires: n.days.filter(|d| *d > 0).map(|d| now() + d * 86_400),
            password,
            max_downloads: n.max_downloads.filter(|m| *m > 0),
            downloads: 0,
            uploads: 0,
        };
        let mut recs = self.file.lock();
        recs.list.push(link.clone());
        self.file
            .save(&mut recs)
            .map_err(|e| FsError(500, e.to_string()))?;
        Ok(link)
    }

    pub fn revoke(&self, key: &str) -> anyhow::Result<bool> {
        let mut recs = self.file.lock();
        let before = recs.list.len();
        recs.list.retain(|l| l.key != key);
        if recs.list.len() == before {
            return Ok(false);
        }
        self.file.save(&mut recs)?;
        self.grants
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|_, (k, _)| k != key);
        Ok(true)
    }

    /// Count one download; false if the link has none left.
    pub fn take_download(&self, key: &str) -> bool {
        let mut recs = self.file.lock();
        let Some(l) = recs.list.iter_mut().find(|l| l.key == key) else {
            return false;
        };
        if l.used_up() {
            return false;
        }
        l.downloads += 1;
        let _ = self.file.save(&mut recs);
        true
    }

    pub fn count_upload(&self, key: &str) {
        let mut recs = self.file.lock();
        if let Some(l) = recs.list.iter_mut().find(|l| l.key == key) {
            l.uploads += 1;
            let _ = self.file.save(&mut recs);
        }
    }

    pub fn unlock(&self, key: &str, password: &str) -> Unlock {
        let Some(link) = self.get(key) else {
            return Unlock::Wrong;
        };
        {
            let mut fails = self.fails.lock().unwrap_or_else(|e| e.into_inner());
            let q = fails.entry(key.to_string()).or_default();
            while q.front().is_some_and(|t| t.elapsed() > FAIL_WINDOW) {
                q.pop_front();
            }
            if q.len() >= MAX_FAILS {
                return Unlock::Wait(FAIL_WINDOW.saturating_sub(q[0].elapsed()));
            }
        }
        let ok = link
            .password
            .as_deref()
            .is_some_and(|h| crate::auth::verify_password(h, password));
        if !ok {
            let mut fails = self.fails.lock().unwrap_or_else(|e| e.into_inner());
            fails
                .entry(key.to_string())
                .or_default()
                .push_back(Instant::now());
            return Unlock::Wrong;
        }
        let grant = crate::auth::random_hex(16);
        let mut grants = self.grants.lock().unwrap_or_else(|e| e.into_inner());
        grants.retain(|_, (_, until)| *until > Instant::now());
        grants.insert(
            grant.clone(),
            (
                key.to_string(),
                Instant::now() + Duration::from_secs(GRANT_SECS),
            ),
        );
        Unlock::Granted(grant)
    }

    pub fn granted(&self, key: &str, grant: &str) -> bool {
        let grants = self.grants.lock().unwrap_or_else(|e| e.into_inner());
        grants
            .get(grant)
            .is_some_and(|(k, until)| k == key && *until > Instant::now())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Root;

    fn cfg(dir: &Path) -> Config {
        let mut c = Config::starter();
        std::fs::create_dir_all(dir.join("a/inbox")).unwrap();
        std::fs::write(dir.join("a/song.mp3"), b"la").unwrap();
        c.roots = vec![Root {
            id: "a".into(),
            name: "A".into(),
            path: dir.join("a"),
            icon: String::new(),
            writable: true,
        }];
        c
    }

    fn new(path: &str, mode: Mode) -> NewLink {
        NewLink {
            path: path.into(),
            mode,
            days: None,
            password: None,
            max_downloads: None,
            label: None,
        }
    }

    #[test]
    fn create_target_limits_and_revoke() {
        let d =
            std::env::temp_dir().join(format!("holodrive-links-{}", crate::auth::random_hex(4)));
        let c = cfg(&d);
        let s = Store::memory();
        let f = s
            .create(
                &c,
                NewLink {
                    max_downloads: Some(1),
                    days: Some(1),
                    ..new("a/song.mp3", Mode::View)
                },
            )
            .unwrap();
        assert!(valid_key(&f.key) && !f.dir && !f.expired());
        assert_eq!(f.target("").unwrap(), "a/song.mp3");
        assert_eq!(f.target("x").unwrap_err().0, 400);
        assert!(s.take_download(&f.key));
        assert!(!s.take_download(&f.key), "only one download allowed");
        assert!(s.get(&f.key).unwrap().used_up());

        let dir = s.create(&c, new("a/inbox", Mode::View)).unwrap();
        assert_eq!(dir.target("/sub/x.txt/").unwrap(), "a/inbox/sub/x.txt");
        assert_eq!(
            s.create(&c, new("a/song.mp3", Mode::Drop)).unwrap_err().0,
            400
        );
        assert_eq!(
            s.create(&c, new("a/../etc", Mode::View)).unwrap_err().0,
            400
        );
        assert_eq!(
            s.create(&c, new("a/missing", Mode::View)).unwrap_err().0,
            404
        );

        assert!(s.revoke(&dir.key).unwrap());
        assert!(s.get(&dir.key).is_none());
        assert!(s.get("../../x").is_none());
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn password_unlock_and_brake() {
        let d =
            std::env::temp_dir().join(format!("holodrive-links-pw-{}", crate::auth::random_hex(4)));
        let c = cfg(&d);
        let s = Store::memory();
        let l = s
            .create(
                &c,
                NewLink {
                    password: Some("hunter2".into()),
                    ..new("a/inbox", Mode::Drop)
                },
            )
            .unwrap();
        assert!(l.has_password());
        let Unlock::Granted(g) = s.unlock(&l.key, "hunter2") else {
            panic!("right password refused")
        };
        assert!(s.granted(&l.key, &g));
        assert!(!s.granted(&l.key, "nope"));
        for _ in 0..MAX_FAILS {
            assert!(matches!(s.unlock(&l.key, "wrong"), Unlock::Wrong));
        }
        assert!(matches!(s.unlock(&l.key, "hunter2"), Unlock::Wait(_)));
        s.revoke(&l.key).unwrap();
        assert!(!s.granted(&l.key, &g), "revoking ends open grants");
        std::fs::remove_dir_all(d).unwrap();
    }
}
