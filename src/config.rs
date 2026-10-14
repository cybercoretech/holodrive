//! `~/.config/holodrive/config.toml`: where to listen, which Host names the
//! tailnet proxy may use, the password hash, and the shared roots.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const DEFAULT_PORT: u16 = 8768;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Root {
    /// URL-safe id (`shared`, `devspace`); the first path segment in the API.
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    #[serde(default = "default_icon")]
    pub icon: String,
    #[serde(default = "yes")]
    pub writable: bool,
}

fn default_icon() -> String {
    "📁".into()
}
fn yes() -> bool {
    true
}
fn default_session_days() -> u64 {
    30
}
fn default_trash_days() -> u64 {
    30
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    pub listen: String,
    /// Extra `Host` values accepted besides 127.0.0.1/localhost, e.g. the
    /// tailnet name `tailscale serve` forwards.
    #[serde(default)]
    pub hosts: Vec<String>,
    #[serde(default)]
    pub password_hash: String,
    /// Trust direct localhost browser access without a password. Keep disabled
    /// by default; this is intended only for a single-user desktop setup.
    #[serde(default)]
    pub local_auth_bypass: bool,
    #[serde(default = "default_session_days")]
    pub session_days: u64,
    #[serde(default = "default_trash_days")]
    pub trash_days: u64,
    #[serde(default)]
    pub roots: Vec<Root>,
}

pub fn config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from(".config"))
        .join("holodrive/config.toml")
}

pub fn cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from(".cache"))
        .join("holodrive")
}

impl Config {
    pub fn load(path: &Path) -> Result<Config> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("can't read {} (run `holodrive init`)", path.display()))?;
        let cfg: Config =
            toml::from_str(&text).with_context(|| format!("bad {}", path.display()))?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = format!(
            "# Holodrive config. `holodrive passwd` sets the password.\n{}",
            toml::to_string_pretty(self)?
        );
        let tmp = path.with_extension("toml.tmp");
        {
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&tmp)?;
            f.write_all(text.as_bytes())?;
        }
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    fn validate(&self) -> Result<()> {
        let mut seen = std::collections::BTreeSet::new();
        for r in &self.roots {
            if r.id.is_empty()
                || !r
                    .id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            {
                bail!("root id {:?} must be letters, digits, - or _", r.id);
            }
            if !seen.insert(r.id.clone()) {
                bail!("duplicate root id {:?}", r.id);
            }
            if !r.path.is_absolute() {
                bail!("root {:?}: path must be absolute", r.id);
            }
        }
        Ok(())
    }

    pub fn port(&self) -> u16 {
        self.listen
            .rsplit(':')
            .next()
            .and_then(|p| p.parse().ok())
            .unwrap_or(DEFAULT_PORT)
    }

    pub fn root(&self, id: &str) -> Option<&Root> {
        self.roots.iter().find(|r| r.id == id)
    }

    /// The starter config: the folders blackbox already shares over Samba.
    pub fn starter() -> Config {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
        let root = |id: &str, name: &str, dir: &str, icon: &str, writable: bool| Root {
            id: id.into(),
            name: name.into(),
            path: home.join(dir),
            icon: icon.into(),
            writable,
        };
        let candidates = [
            root("shared", "Shared", "Shared", "🛰️", true),
            root("devspace", "Devspace", "Devspace", "🦀", true),
            root("webdev", "Webdev", "Webdev", "🌐", true),
            root("animations", "Animations", "Animations", "🎬", true),
            root("documents", "Documents", "Documents", "📚", true),
            root("pictures", "Pictures", "Pictures", "🖼️", true),
            root("downloads", "Downloads", "Downloads", "📥", true),
            root("wiki", "Wiki", "Wiki", "🧠", true),
            root("isos", "ISOs", "ISOs", "💿", false),
        ];
        Config {
            listen: format!("127.0.0.1:{DEFAULT_PORT}"),
            hosts: vec![],
            password_hash: String::new(),
            local_auth_bypass: false,
            session_days: default_session_days(),
            trash_days: default_trash_days(),
            roots: candidates.into_iter().filter(|r| r.path.is_dir()).collect(),
        }
    }
}
