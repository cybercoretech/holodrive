//! Time machine: browse and restore from snapper snapshots.
//!
//! Needs no privileges when snapper lets the user list a config
//! (`ALLOW_USERS`) and `SYNC_ACL=yes` makes `.snapshots` readable. A drive
//! is covered by the snapper config whose subvolume contains it; a path's
//! past self lives at `<subvolume>/.snapshots/<n>/snapshot/<same relative
//! path>`. Everything read from a snapshot must stay inside the drive's own
//! place in that snapshot (symlinks included). Snapshots are never written.

use crate::config::{Config, Root};
use crate::fsops::{self, FsError, FsResult};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Clone)]
struct SnapConfig {
    name: String,
    subvolume: PathBuf,
}

static CONFIGS: Mutex<Option<(Instant, Vec<SnapConfig>)>> = Mutex::new(None);

fn snapper(args: &[&str]) -> FsResult<Value> {
    let out = Command::new("snapper")
        .arg("--jsonout")
        .args(args)
        .output()
        .map_err(|e| FsError(503, format!("snapper: {e}")))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(FsError(
            503,
            format!("snapper: {}", err.lines().last().unwrap_or("failed").trim()),
        ));
    }
    serde_json::from_slice(&out.stdout).map_err(|e| FsError(503, format!("snapper output: {e}")))
}

fn configs() -> Vec<SnapConfig> {
    let mut cache = CONFIGS.lock().unwrap();
    if let Some((at, list)) = cache.as_ref()
        && at.elapsed() < Duration::from_secs(120)
    {
        return list.clone();
    }
    let list: Vec<SnapConfig> = snapper(&["list-configs"])
        .ok()
        .and_then(|v| v["configs"].as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|c| {
            Some(SnapConfig {
                name: c["config"].as_str()?.to_string(),
                subvolume: PathBuf::from(c["subvolume"].as_str()?)
                    .canonicalize()
                    .ok()?,
            })
        })
        .collect();
    *cache = Some((Instant::now(), list.clone()));
    list
}

/// The config covering `real` (deepest subvolume wins).
fn config_for(real: &Path) -> Option<SnapConfig> {
    configs()
        .into_iter()
        .filter(|c| real.starts_with(&c.subvolume) && c.subvolume != Path::new("/"))
        .max_by_key(|c| c.subvolume.components().count())
}

fn snap_number(n: &str) -> FsResult<u32> {
    n.parse::<u32>()
        .ok()
        .filter(|n| *n > 0)
        .ok_or_else(|| FsError(400, "bad snapshot number".into()))
}

struct Located<'a> {
    root: &'a Root,
    config: SnapConfig,
    /// Live path relative to the subvolume.
    rel_sub: PathBuf,
    /// The drive's own position relative to the subvolume.
    root_sub: PathBuf,
    rel: PathBuf,
}

fn locate<'a>(cfg: &'a Config, vpath: &str) -> FsResult<Located<'a>> {
    let r = fsops::resolve(cfg, vpath)?;
    let root_real = r.root.path.canonicalize()?;
    let config = config_for(&root_real)
        .ok_or_else(|| FsError(404, format!("{} has no snapper snapshots", r.root.name)))?;
    let root_sub = root_real
        .strip_prefix(&config.subvolume)
        .unwrap_or(Path::new(""))
        .to_path_buf();
    Ok(Located {
        root: r.root,
        rel_sub: root_sub.join(&r.rel),
        root_sub,
        rel: r.rel,
        config,
    })
}

impl Located<'_> {
    fn base(&self, n: u32) -> PathBuf {
        self.config
            .subvolume
            .join(".snapshots")
            .join(n.to_string())
            .join("snapshot")
    }
    /// The path inside snapshot `n`, checked to stay inside the drive's
    /// place in that snapshot. `None` if it didn't exist then.
    fn in_snapshot(&self, n: u32) -> FsResult<Option<PathBuf>> {
        let base = self.base(n);
        let fence = base.join(&self.root_sub);
        let p = base.join(&self.rel_sub);
        if std::fs::symlink_metadata(&p).is_err() {
            return Ok(None);
        }
        let real = p.canonicalize()?;
        let fence_real = fence.canonicalize().unwrap_or(fence);
        if !real.starts_with(&fence_real) {
            return Err(FsError(
                403,
                "that link points outside the drive's snapshot".into(),
            ));
        }
        Ok(Some(p))
    }
}

fn snapshot_list(config: &str) -> FsResult<Vec<Value>> {
    let v = snapper(&["-c", config, "list", "--disable-used-space"])?;
    let list = v[config].as_array().cloned().unwrap_or_default();
    let mut out: Vec<Value> = list
        .into_iter()
        .filter(|s| s["number"].as_u64().unwrap_or(0) > 0)
        .map(|s| {
            json!({
                "number": s["number"], "date": s["date"], "type": s["type"],
                "description": s["description"], "cleanup": s["cleanup"],
            })
        })
        .collect();
    out.sort_by_key(|s| std::cmp::Reverse(s["number"].as_u64().unwrap_or(0)));
    Ok(out)
}

/// Snapshots covering a path's drive (newest first).
pub fn timeline(cfg: &Config, vpath: &str) -> FsResult<Value> {
    let l = locate(cfg, vpath)?;
    let snaps = snapshot_list(&l.config.name)?;
    Ok(json!({ "config": l.config.name, "drive": l.root.id, "snapshots": snaps }))
}

/// A folder as it was in snapshot `n`, plus which entries are gone now.
pub fn list(cfg: &Config, vpath: &str, n: &str) -> FsResult<Value> {
    let n = snap_number(n)?;
    let l = locate(cfg, vpath)?;
    let dir = l
        .in_snapshot(n)?
        .ok_or_else(|| FsError(404, "that folder didn't exist in this snapshot".into()))?;
    if !dir.is_dir() {
        return Err(FsError(400, "not a folder".into()));
    }
    let fence = l.base(n).join(&l.root_sub).canonicalize()?;
    let live = l.root.path.join(&l.rel);
    let mut entries: Vec<Value> = std::fs::read_dir(&dir)?
        .flatten()
        .filter_map(|d| {
            let name = d.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                return None;
            }
            let e = fsops::entry_for(&fence, &d.path(), name.clone())?;
            let mut v = serde_json::to_value(e).ok()?;
            let now = std::fs::symlink_metadata(live.join(&name)).ok();
            v["gone"] = json!(now.is_none());
            v["changed"] = json!(now.as_ref().is_some_and(|m| m.is_file()
                && (m.len() != v["size"].as_u64().unwrap_or(0)
                    || mtime(m) != v["mtime"].as_u64().unwrap_or(0))));
            Some(v)
        })
        .collect();
    entries.sort_by(|a, b| {
        (a["kind"] != "dir")
            .cmp(&(b["kind"] != "dir"))
            .then_with(|| {
                a["name"]
                    .as_str()
                    .unwrap_or("")
                    .to_lowercase()
                    .cmp(&b["name"].as_str().unwrap_or("").to_lowercase())
            })
    });
    Ok(json!({
        "path": fsops::join_vpath(&l.root.id, &l.rel), "root": l.root.id, "writable": false,
        "snapshot": n, "entries": entries, "live_exists": live.exists(),
    }))
}

fn mtime(m: &std::fs::Metadata) -> u64 {
    m.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Every distinct version of a file across snapshots (newest first).
pub fn versions(cfg: &Config, vpath: &str) -> FsResult<Value> {
    let l = locate(cfg, vpath)?;
    let snaps = snapshot_list(&l.config.name)?;
    let live = std::fs::metadata(l.root.path.join(&l.rel)).ok();
    let mut out = vec![];
    let mut last: Option<(u64, u64)> = live.as_ref().map(|m| (m.len(), mtime(m)));
    for s in snaps.iter().take(400) {
        let n = s["number"].as_u64().unwrap_or(0) as u32;
        let Ok(Some(p)) = l.in_snapshot(n) else {
            continue;
        };
        let Ok(m) = std::fs::metadata(&p) else {
            continue;
        };
        if !m.is_file() {
            continue;
        }
        let key = (m.len(), mtime(&m));
        if Some(key) == last {
            continue; // same as the newer version already listed
        }
        last = Some(key);
        out.push(json!({ "snapshot": n, "date": s["date"], "size": key.0, "mtime": key.1 }));
    }
    Ok(json!({
        "path": fsops::join_vpath(&l.root.id, &l.rel),
        "current": live.map(|m| json!({ "size": m.len(), "mtime": mtime(&m) })),
        "versions": out,
    }))
}

/// Absolute path of `vpath` in snapshot `n` (for raw/thumb/text).
pub fn file(cfg: &Config, vpath: &str, n: &str) -> FsResult<PathBuf> {
    let n = snap_number(n)?;
    let l = locate(cfg, vpath)?;
    l.in_snapshot(n)?
        .ok_or_else(|| FsError(404, "not in that snapshot".into()))
}

pub fn read_text(cfg: &Config, vpath: &str, n: &str) -> FsResult<Value> {
    let p = file(cfg, vpath, n)?;
    let m = std::fs::metadata(&p)?;
    if !m.is_file() || m.len() > 4 * 1024 * 1024 {
        return Err(FsError(413, "not a text file this size".into()));
    }
    let text =
        String::from_utf8(std::fs::read(&p)?).map_err(|_| FsError(415, "not UTF-8 text".into()))?;
    Ok(json!({ "text": text, "mtime": mtime(&m), "writable": false }))
}

/// Bring `vpath` back from snapshot `n`. `replace` = put it back in place
/// (the current one goes to Holodrive's trash first); otherwise it lands
/// next to it as "name (from 2026-10-13 04.00).ext".
pub fn restore(cfg: &Config, vpath: &str, n: &str, replace: bool, label: &str) -> FsResult<Value> {
    let num = snap_number(n)?;
    let l = locate(cfg, vpath)?;
    if !l.root.writable {
        return Err(FsError(403, format!("{} is read-only", l.root.name)));
    }
    if l.rel.as_os_str().is_empty() {
        return Err(FsError(
            400,
            "restore folders inside a drive, not the whole drive".into(),
        ));
    }
    let src = l
        .in_snapshot(num)?
        .ok_or_else(|| FsError(404, "not in that snapshot".into()))?;
    let live = l.root.path.join(&l.rel);
    let parent = live.parent().unwrap().to_path_buf();
    std::fs::create_dir_all(&parent)?;
    let name = live.file_name().unwrap().to_string_lossy().into_owned();
    let mut trashed = Value::Null;
    let target_name = if replace {
        if std::fs::symlink_metadata(&live).is_ok() {
            trashed = json!(fsops::trash(cfg, &[fsops::join_vpath(&l.root.id, &l.rel)])?);
        }
        name.clone()
    } else {
        let p = Path::new(&name);
        let tag: String = label
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || " -.".contains(*c))
            .take(24)
            .collect();
        let suffix = format!(
            " (from {})",
            if tag.is_empty() {
                format!("snapshot {num}")
            } else {
                tag
            }
        );
        let candidate = match (p.file_stem(), p.extension()) {
            (Some(s), Some(e)) => {
                format!("{}{suffix}.{}", s.to_string_lossy(), e.to_string_lossy())
            }
            _ => format!("{name}{suffix}"),
        };
        fsops::unique_name(&parent, &candidate)
    };
    fsops::copy_recursive(&src, &parent.join(&target_name))?;
    let rel_parent = l.rel.parent().map(Path::to_path_buf).unwrap_or_default();
    Ok(
        json!({ "path": fsops::join_vpath(&l.root.id, &rel_parent.join(target_name)), "trashed": trashed }),
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn snapshot_numbers() {
        assert!(super::snap_number("12").is_ok());
        for bad in ["0", "-1", "../3", "3/../../x", ""] {
            assert!(super::snap_number(bad).is_err(), "{bad}");
        }
    }
}
