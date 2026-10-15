//! Resumable uploads: a file arrives in chunks and survives dropped
//! connections, reloads and restarts.
//!
//! `start` records the destination and size and returns an id. Chunks are
//! appended to `<root>/.holodrive-uploads/<id>.part` (same filesystem as the
//! drive, so finishing is a rename); each one says the offset it starts at,
//! and a mismatch answers 409 with the offset the server has, so a client
//! that lost track just continues from there. `finish` picks the final name
//! (no clobbering, like single-shot uploads) and moves the file into place.
//! Uploads untouched for `STALE_SECS` are swept.

use crate::config::{Config, Root};
use crate::fsops::{self, FsError, FsResult};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

pub const UPLOAD_DIR: &str = ".holodrive-uploads";
/// What clients should send per request; the server accepts any size.
pub const CHUNK: u64 = 8 * 1024 * 1024;
const STALE_SECS: u64 = 24 * 3600;

#[derive(Debug, Serialize, Deserialize)]
struct Meta {
    dir: String,
    name: String,
    size: u64,
    overwrite: bool,
}

/// An upload found by id: where its data lives and where it's going.
#[derive(Debug)]
pub struct Upload {
    pub part: PathBuf,
    meta_path: PathBuf,
    meta: Meta,
}

impl Upload {
    pub fn size(&self) -> u64 {
        self.meta.size
    }
    pub fn received(&self) -> FsResult<u64> {
        Ok(std::fs::metadata(&self.part)?.len())
    }
}

fn valid_id(id: &str) -> bool {
    id.len() == 32
        && id
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

fn dir_of(root: &Root) -> PathBuf {
    root.path.join(UPLOAD_DIR)
}

pub fn start(cfg: &Config, dir: &str, name: &str, size: u64, overwrite: bool) -> FsResult<Value> {
    let d = fsops::resolve(cfg, dir)?;
    d.writable()?;
    let parts: Vec<&str> = name.split('/').filter(|p| !p.is_empty()).collect();
    if parts.is_empty() {
        return Err(FsError(400, "missing file name".into()));
    }
    for p in &parts {
        fsops::check_name(p)?;
    }
    if let Some((_, free)) = fsops::disk(&d.root.path)
        && size > free
    {
        return Err(FsError(
            507,
            format!("not enough space on {} for this file", d.root.name),
        ));
    }
    let updir = dir_of(d.root);
    std::fs::create_dir_all(&updir)?;
    let id = crate::auth::random_hex(16);
    let meta = Meta {
        dir: d.vpath(),
        name: parts.join("/"),
        size,
        overwrite,
    };
    std::fs::File::create(updir.join(format!("{id}.part")))?;
    std::fs::write(
        updir.join(format!("{id}.json")),
        serde_json::to_vec(&meta).map_err(|e| FsError(500, e.to_string()))?,
    )?;
    Ok(json!({ "id": id, "offset": 0, "size": size, "chunk": CHUNK }))
}

pub fn find(cfg: &Config, id: &str) -> FsResult<Upload> {
    if !valid_id(id) {
        return Err(FsError(400, "bad upload id".into()));
    }
    for r in &cfg.roots {
        let meta_path = dir_of(r).join(format!("{id}.json"));
        let Ok(bytes) = std::fs::read(&meta_path) else {
            continue;
        };
        let meta: Meta = serde_json::from_slice(&bytes)
            .map_err(|e| FsError(500, format!("upload record unreadable: {e}")))?;
        return Ok(Upload {
            part: dir_of(r).join(format!("{id}.part")),
            meta_path,
            meta,
        });
    }
    Err(FsError(
        404,
        "unknown or expired upload — start it again".into(),
    ))
}

pub fn status(cfg: &Config, id: &str) -> FsResult<Value> {
    let u = find(cfg, id)?;
    Ok(json!({ "offset": u.received()?, "size": u.size() }))
}

pub fn finish(cfg: &Config, id: &str) -> FsResult<Value> {
    let u = find(cfg, id)?;
    let got = u.received()?;
    if got != u.size() {
        return Err(FsError(
            409,
            format!("incomplete: {got} of {} bytes", u.size()),
        ));
    }
    let (path, vpath) = fsops::upload_target(cfg, &u.meta.dir, &u.meta.name, u.meta.overwrite)?;
    move_into_place(&u.part, &path)?;
    let _ = std::fs::remove_file(&u.meta_path);
    Ok(json!({ "path": vpath, "size": got }))
}

/// Rename; if a subfolder is another filesystem, copy beside the target
/// first so the target still only ever appears complete.
fn move_into_place(from: &Path, to: &Path) -> FsResult<()> {
    match std::fs::rename(from, to) {
        Err(e) if e.raw_os_error() == Some(libc::EXDEV) => {
            let tmp =
                to.with_file_name(format!(".holodrive-upload-{}", crate::auth::random_hex(6)));
            let copied = std::fs::copy(from, &tmp).and_then(|_| std::fs::rename(&tmp, to));
            if let Err(e) = copied {
                let _ = std::fs::remove_file(&tmp);
                return Err(e.into());
            }
            let _ = std::fs::remove_file(from);
            Ok(())
        }
        r => Ok(r?),
    }
}

pub fn cancel(cfg: &Config, id: &str) -> FsResult<Value> {
    let u = find(cfg, id)?;
    let _ = std::fs::remove_file(&u.part);
    std::fs::remove_file(&u.meta_path)?;
    Ok(json!({ "ok": true }))
}

/// Remove uploads nobody has touched for a day.
pub fn sweep(cfg: &Config) -> usize {
    let cutoff = SystemTime::now() - Duration::from_secs(STALE_SECS);
    let stale = |p: &Path| {
        std::fs::metadata(p)
            .and_then(|m| m.modified())
            .map(|t| t < cutoff)
            .unwrap_or(false)
    };
    let mut n = 0;
    for r in &cfg.roots {
        let Ok(rd) = std::fs::read_dir(dir_of(r)) else {
            continue;
        };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let Some(id) = name.strip_suffix(".json") else {
                // A .part without its record is junk once it's old.
                if name.ends_with(".part")
                    && !e.path().with_extension("json").exists()
                    && stale(&e.path())
                    && std::fs::remove_file(e.path()).is_ok()
                {
                    n += 1;
                }
                continue;
            };
            let part = dir_of(r).join(format!("{id}.part"));
            let touched = if part.exists() { &part } else { &e.path() };
            if stale(touched) {
                let _ = std::fs::remove_file(&part);
                if std::fs::remove_file(e.path()).is_ok() {
                    n += 1;
                }
            }
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Root;
    use std::io::Write;

    fn cfg(dir: &Path) -> Config {
        let mut c = Config::starter();
        std::fs::create_dir_all(dir.join("a")).unwrap();
        c.roots = vec![Root {
            id: "a".into(),
            name: "A".into(),
            path: dir.join("a"),
            icon: String::new(),
            writable: true,
        }];
        c
    }

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "holodrive-uploads-{tag}-{}",
            crate::auth::random_hex(4)
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn append(cfg: &Config, id: &str, bytes: &[u8]) {
        let u = find(cfg, id).unwrap();
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&u.part)
            .unwrap();
        f.write_all(bytes).unwrap();
    }

    #[test]
    fn start_append_finish_lands_without_clobbering() {
        let d = tmpdir("finish");
        let c = cfg(&d);
        std::fs::write(d.join("a/clip.mp4"), b"old").unwrap();
        let s = start(&c, "a", "sub/clip.mp4", 6, false).unwrap();
        let id = s["id"].as_str().unwrap().to_string();
        append(&c, &id, b"abc");
        assert_eq!(status(&c, &id).unwrap()["offset"], 3);
        assert_eq!(finish(&c, &id).unwrap_err().0, 409);
        append(&c, &id, b"def");
        let done = finish(&c, &id).unwrap();
        assert_eq!(done["path"], "a/sub/clip.mp4");
        assert_eq!(std::fs::read(d.join("a/sub/clip.mp4")).unwrap(), b"abcdef");
        assert_eq!(find(&c, &id).unwrap_err().0, 404);

        let s = start(&c, "a", "clip.mp4", 1, false).unwrap();
        let id = s["id"].as_str().unwrap().to_string();
        append(&c, &id, b"x");
        assert_eq!(finish(&c, &id).unwrap()["path"], "a/clip (2).mp4");
        assert_eq!(std::fs::read(d.join("a/clip.mp4")).unwrap(), b"old");
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn refuses_bad_names_ids_and_oversize() {
        let d = tmpdir("refuse");
        let c = cfg(&d);
        assert_eq!(start(&c, "a", "../x", 1, false).unwrap_err().0, 400);
        assert_eq!(start(&c, "a", ".env", 1, false).unwrap_err().0, 400);
        assert_eq!(start(&c, "a", "", 1, false).unwrap_err().0, 400);
        assert_eq!(start(&c, "a", "big", u64::MAX, false).unwrap_err().0, 507);
        assert_eq!(find(&c, "../../etc/passwd").unwrap_err().0, 400);
        assert_eq!(find(&c, &"A".repeat(32)).unwrap_err().0, 400);
        assert_eq!(find(&c, &"0".repeat(32)).unwrap_err().0, 404);
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn cancel_and_sweep_remove_the_data() {
        let d = tmpdir("sweep");
        let c = cfg(&d);
        let id = start(&c, "a", "f", 4, false).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        cancel(&c, &id).unwrap();
        assert!(!d.join(format!("a/{UPLOAD_DIR}/{id}.part")).exists());

        let id = start(&c, "a", "g", 4, false).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(sweep(&c), 0, "fresh uploads stay");
        let old = SystemTime::now() - Duration::from_secs(STALE_SECS + 60);
        let part = d.join(format!("a/{UPLOAD_DIR}/{id}.part"));
        std::fs::File::options()
            .write(true)
            .open(&part)
            .unwrap()
            .set_modified(old)
            .unwrap();
        assert_eq!(sweep(&c), 1);
        assert!(!part.exists());
        std::fs::remove_dir_all(d).unwrap();
    }
}
