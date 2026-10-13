//! Everything that touches the shared folders.
//!
//! Paths in the API are virtual: `<root id>/<relative path>`. `resolve`
//! is the only way in. It refuses `..`, hidden components (dot files hold
//! `.git`, `.env`, `.snapshots` and our own trash) and anything whose real
//! location (after symlinks) is outside the root.
//!
//! Deleting never destroys: items move to `<root>/.holodrive-trash/<id>/`
//! on the same filesystem (instant, undoable) and are purged after
//! `trash_days`.

use crate::config::{Config, Root};
use serde::Serialize;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const TRASH_DIR: &str = ".holodrive-trash";
const TEXT_LIMIT: u64 = 4 * 1024 * 1024;

#[derive(Debug)]
pub struct FsError(pub u16, pub String);

impl FsError {
    fn bad(msg: impl Into<String>) -> FsError {
        FsError(400, msg.into())
    }
    fn io(e: std::io::Error) -> FsError {
        use std::io::ErrorKind::*;
        let code = match e.kind() {
            NotFound => 404,
            PermissionDenied => 403,
            AlreadyExists => 409,
            _ => 500,
        };
        FsError(code, e.to_string())
    }
}

impl From<std::io::Error> for FsError {
    fn from(e: std::io::Error) -> FsError {
        FsError::io(e)
    }
}

pub type FsResult<T> = Result<T, FsError>;

pub struct Resolved<'a> {
    pub root: &'a Root,
    pub rel: PathBuf,
    pub abs: PathBuf,
}

impl Resolved<'_> {
    pub fn vpath(&self) -> String {
        join_vpath(&self.root.id, &self.rel)
    }
    pub fn writable(&self) -> FsResult<()> {
        if self.root.writable {
            Ok(())
        } else {
            Err(FsError(403, format!("{} is read-only", self.root.name)))
        }
    }
}

pub fn join_vpath(root: &str, rel: &Path) -> String {
    let rel = rel.to_string_lossy();
    if rel.is_empty() {
        root.to_string()
    } else {
        format!("{root}/{rel}")
    }
}

/// A single file or folder name typed by the user.
pub fn check_name(name: &str) -> FsResult<()> {
    if name.is_empty() || name.len() > 255 {
        return Err(FsError::bad("name must be 1–255 bytes"));
    }
    if name.contains('/') || name.contains('\0') {
        return Err(FsError::bad("name can't contain / "));
    }
    if name == "." || name == ".." || name.starts_with('.') {
        return Err(FsError::bad(
            "hidden names (starting with .) aren't allowed here",
        ));
    }
    Ok(())
}

pub fn resolve<'a>(cfg: &'a Config, vpath: &str) -> FsResult<Resolved<'a>> {
    let mut parts = vpath.split('/').filter(|p| !p.is_empty());
    let root_id = parts.next().ok_or_else(|| FsError::bad("empty path"))?;
    let root = cfg
        .root(root_id)
        .ok_or_else(|| FsError(404, format!("no root {root_id:?}")))?;
    let mut rel = PathBuf::new();
    for p in parts {
        check_name(p)?;
        rel.push(p);
    }
    let abs = root.path.join(&rel);
    contained(&root.path, &abs)?;
    Ok(Resolved { root, rel, abs })
}

/// `abs` (or, if it doesn't exist yet, its nearest existing ancestor) must
/// really live inside `root` once symlinks are followed.
fn contained(root: &Path, abs: &Path) -> FsResult<()> {
    let real_root = root
        .canonicalize()
        .map_err(|e| FsError(503, format!("root {} unavailable: {e}", root.display())))?;
    let mut probe = abs.to_path_buf();
    loop {
        match probe.canonicalize() {
            Ok(real) => {
                return if real.starts_with(&real_root) {
                    Ok(())
                } else {
                    Err(FsError(
                        403,
                        "that link points outside the shared folder".into(),
                    ))
                };
            }
            Err(_) if probe.pop() => continue,
            Err(e) => return Err(FsError::io(e)),
        }
    }
}

fn secs(t: std::io::Result<SystemTime>) -> u64 {
    t.ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[derive(Serialize)]
pub struct Entry {
    pub name: String,
    pub kind: &'static str,
    pub size: u64,
    pub mtime: u64,
    pub ext: String,
    pub mime: String,
    pub link: bool,
    pub escapes: bool,
    pub mode: u32,
}

pub fn entry_for(root_real: &Path, path: &Path, name: String) -> Option<Entry> {
    use std::os::unix::fs::PermissionsExt;
    let lmeta = std::fs::symlink_metadata(path).ok()?;
    let link = lmeta.file_type().is_symlink();
    let meta = if link {
        std::fs::metadata(path).ok()
    } else {
        Some(lmeta.clone())
    };
    let escapes = link
        && path
            .canonicalize()
            .map(|r| !r.starts_with(root_real))
            .unwrap_or(true);
    let (kind, size, mtime, mode) = match &meta {
        Some(m) if m.is_dir() => ("dir", 0, secs(m.modified()), m.permissions().mode()),
        Some(m) if m.is_file() => ("file", m.len(), secs(m.modified()), m.permissions().mode()),
        Some(m) => ("other", 0, secs(m.modified()), m.permissions().mode()),
        None => (
            "other",
            0,
            secs(lmeta.modified()),
            lmeta.permissions().mode(),
        ),
    };
    let ext = Path::new(&name)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let mime = if kind == "file" {
        mime_guess::from_path(&name)
            .first_or_octet_stream()
            .essence_str()
            .to_string()
    } else {
        String::new()
    };
    Some(Entry {
        name,
        kind,
        size,
        mtime,
        ext,
        mime,
        link,
        escapes,
        mode,
    })
}

pub fn list(cfg: &Config, vpath: &str) -> FsResult<Value> {
    let r = resolve(cfg, vpath)?;
    let meta = std::fs::metadata(&r.abs)?;
    if !meta.is_dir() {
        return Err(FsError::bad("not a folder"));
    }
    let root_real = r.root.path.canonicalize()?;
    let mut entries: Vec<Entry> = std::fs::read_dir(&r.abs)?
        .filter_map(|d| d.ok())
        .filter_map(|d| {
            let name = d.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                return None;
            }
            entry_for(&root_real, &d.path(), name)
        })
        .collect();
    entries.sort_by(|a, b| {
        (a.kind != "dir")
            .cmp(&(b.kind != "dir"))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(json!({
        "path": r.vpath(),
        "root": r.root.id,
        "writable": r.root.writable,
        "mtime": secs(meta.modified()),
        "entries": entries,
    }))
}

pub fn stat(cfg: &Config, vpath: &str) -> FsResult<Value> {
    let r = resolve(cfg, vpath)?;
    let root_real = r.root.path.canonicalize()?;
    let name = r
        .rel
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| r.root.name.clone());
    let e = entry_for(&root_real, &r.abs, name).ok_or_else(|| FsError(404, "not found".into()))?;
    let mut v = serde_json::to_value(e).unwrap_or_default();
    v["path"] = json!(r.vpath());
    v["abs"] = json!(r.abs.to_string_lossy());
    v["writable"] = json!(r.root.writable);
    Ok(v)
}

pub fn disk(path: &Path) -> Option<(u64, u64)> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let c = CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut s) } != 0 {
        return None;
    }
    let bs = s.f_frsize as u64;
    Some((s.f_blocks as u64 * bs, s.f_bavail as u64 * bs))
}

pub fn roots(cfg: &Config) -> Value {
    let list: Vec<Value> = cfg
        .roots
        .iter()
        .map(|r| {
            let (total, free) = disk(&r.path).unwrap_or((0, 0));
            json!({
                "id": r.id, "name": r.name, "icon": r.icon, "writable": r.writable,
                "online": r.path.is_dir(), "total": total, "free": free,
            })
        })
        .collect();
    json!(list)
}

/// `name`, or `name (2).ext`, `name (3).ext` … whichever is free in `dir`.
pub fn unique_name(dir: &Path, name: &str) -> String {
    if !dir.join(name).exists() && std::fs::symlink_metadata(dir.join(name)).is_err() {
        return name.to_string();
    }
    let p = Path::new(name);
    let (stem, ext) = match (p.file_stem(), p.extension()) {
        (Some(s), Some(e)) if !name.starts_with('.') => (
            s.to_string_lossy().into_owned(),
            format!(".{}", e.to_string_lossy()),
        ),
        _ => (name.to_string(), String::new()),
    };
    (2..)
        .map(|n| format!("{stem} ({n}){ext}"))
        .find(|c| std::fs::symlink_metadata(dir.join(c)).is_err())
        .unwrap()
}

pub fn mkdir(cfg: &Config, dir: &str, name: &str) -> FsResult<String> {
    check_name(name)?;
    let r = resolve(cfg, dir)?;
    r.writable()?;
    let target = r.abs.join(name);
    std::fs::create_dir(&target)?;
    Ok(join_vpath(&r.root.id, &r.rel.join(name)))
}

pub fn rename(cfg: &Config, vpath: &str, new_name: &str) -> FsResult<String> {
    check_name(new_name)?;
    let r = resolve(cfg, vpath)?;
    r.writable()?;
    if r.rel.as_os_str().is_empty() {
        return Err(FsError::bad("can't rename a root"));
    }
    let parent = r.abs.parent().unwrap().to_path_buf();
    let target = parent.join(new_name);
    if std::fs::symlink_metadata(&target).is_ok() {
        return Err(FsError(409, format!("{new_name} already exists")));
    }
    std::fs::rename(&r.abs, &target)?;
    Ok(join_vpath(&r.root.id, &r.rel.with_file_name(new_name)))
}

pub fn copy_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    let meta = std::fs::symlink_metadata(src)?;
    if meta.file_type().is_symlink() {
        std::os::unix::fs::symlink(std::fs::read_link(src)?, dst)
    } else if meta.is_dir() {
        std::fs::create_dir(dst)?;
        for e in std::fs::read_dir(src)? {
            let e = e?;
            copy_recursive(&e.path(), &dst.join(e.file_name()))?;
        }
        std::fs::set_permissions(dst, meta.permissions())
    } else {
        std::fs::copy(src, dst).map(|_| ())
    }
}

fn move_path(src: &Path, dst: &Path) -> std::io::Result<()> {
    match std::fs::rename(src, dst) {
        Err(e) if e.raw_os_error() == Some(libc::EXDEV) => {
            copy_recursive(src, dst)?;
            if std::fs::symlink_metadata(src)?.is_dir() {
                std::fs::remove_dir_all(src)
            } else {
                std::fs::remove_file(src)
            }
        }
        other => other,
    }
}

/// Move or copy `sources` into the folder `dest`. Name clashes get a
/// ` (2)` suffix instead of overwriting.
pub fn transfer(cfg: &Config, sources: &[String], dest: &str, copy: bool) -> FsResult<Vec<String>> {
    let d = resolve(cfg, dest)?;
    d.writable()?;
    if !d.abs.is_dir() {
        return Err(FsError::bad("destination isn't a folder"));
    }
    let mut out = vec![];
    for s in sources {
        let r = resolve(cfg, s)?;
        if !copy {
            r.writable()?;
        }
        if r.rel.as_os_str().is_empty() {
            return Err(FsError::bad("can't move or copy a whole root"));
        }
        if d.abs.starts_with(&r.abs) {
            return Err(FsError::bad("can't put a folder inside itself"));
        }
        let name = r.abs.file_name().unwrap().to_string_lossy().into_owned();
        if !copy && r.abs.parent() == Some(d.abs.as_path()) {
            out.push(r.vpath());
            continue;
        }
        let final_name = unique_name(&d.abs, &name);
        let target = d.abs.join(&final_name);
        if copy {
            copy_recursive(&r.abs, &target)?;
        } else {
            move_path(&r.abs, &target)?;
        }
        out.push(join_vpath(&d.root.id, &d.rel.join(final_name)));
    }
    Ok(out)
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn trash(cfg: &Config, paths: &[String]) -> FsResult<Vec<Value>> {
    let mut out = vec![];
    for p in paths {
        let r = resolve(cfg, p)?;
        r.writable()?;
        if r.rel.as_os_str().is_empty() {
            return Err(FsError::bad("can't delete a root"));
        }
        let id = format!("{}-{}", now(), crate::auth::random_hex(4));
        let slot = r.root.path.join(TRASH_DIR).join(&id);
        std::fs::create_dir_all(&slot)?;
        let name = r.abs.file_name().unwrap().to_string_lossy().into_owned();
        let from = r.rel.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        let info = json!({ "name": name, "from": from.to_string_lossy(), "deleted": now() });
        std::fs::write(slot.join("info.json"), info.to_string())?;
        if let Err(e) = move_path(&r.abs, &slot.join("item")) {
            let _ = std::fs::remove_dir_all(&slot);
            return Err(e.into());
        }
        out.push(json!({ "root": r.root.id, "id": id, "name": name }));
    }
    Ok(out)
}

fn trash_slot(cfg: &Config, root: &str, id: &str) -> FsResult<(PathBuf, Value)> {
    let r = cfg
        .root(root)
        .ok_or_else(|| FsError(404, "no such root".into()))?;
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
        return Err(FsError::bad("bad trash id"));
    }
    let slot = r.path.join(TRASH_DIR).join(id);
    let info: Value = serde_json::from_str(&std::fs::read_to_string(slot.join("info.json"))?)
        .map_err(|e| FsError(500, e.to_string()))?;
    Ok((slot, info))
}

pub fn trash_list(cfg: &Config) -> Value {
    let mut items = vec![];
    for r in &cfg.roots {
        let Ok(rd) = std::fs::read_dir(r.path.join(TRASH_DIR)) else {
            continue;
        };
        for e in rd.flatten() {
            let id = e.file_name().to_string_lossy().into_owned();
            let Ok((slot, info)) = trash_slot(cfg, &r.id, &id) else {
                continue;
            };
            let item = slot.join("item");
            let meta = std::fs::symlink_metadata(&item).ok();
            items.push(json!({
                "root": r.id, "root_name": r.name, "root_icon": r.icon, "id": id,
                "name": info["name"], "from": info["from"], "deleted": info["deleted"],
                "kind": if meta.as_ref().is_some_and(|m| m.is_dir()) { "dir" } else { "file" },
                "size": meta.map(|m| m.len()).unwrap_or(0),
            }));
        }
    }
    items.sort_by_key(|v| std::cmp::Reverse(v["deleted"].as_u64().unwrap_or(0)));
    json!(items)
}

pub fn restore(cfg: &Config, root: &str, id: &str) -> FsResult<String> {
    let (slot, info) = trash_slot(cfg, root, id)?;
    let r = cfg.root(root).unwrap();
    let from = PathBuf::from(info["from"].as_str().unwrap_or(""));
    for c in from.components() {
        check_name(&c.as_os_str().to_string_lossy())?;
    }
    let parent = r.path.join(&from);
    std::fs::create_dir_all(&parent)?;
    contained(&r.path, &parent)?;
    let name = unique_name(&parent, info["name"].as_str().unwrap_or("restored"));
    move_path(&slot.join("item"), &parent.join(&name))?;
    std::fs::remove_dir_all(&slot)?;
    Ok(join_vpath(root, &from.join(name)))
}

pub fn purge(cfg: &Config, root: &str, id: &str) -> FsResult<()> {
    let (slot, _) = trash_slot(cfg, root, id)?;
    std::fs::remove_dir_all(slot)?;
    Ok(())
}

/// Drop trash older than `days`; returns how many items went.
pub fn purge_old(cfg: &Config, days: u64) -> usize {
    let cutoff = now().saturating_sub(days * 86_400);
    let mut n = 0;
    for r in &cfg.roots {
        let Ok(rd) = std::fs::read_dir(r.path.join(TRASH_DIR)) else {
            continue;
        };
        for e in rd.flatten() {
            let id = e.file_name().to_string_lossy().into_owned();
            let deleted = trash_slot(cfg, &r.id, &id)
                .ok()
                .and_then(|(_, i)| i["deleted"].as_u64())
                .unwrap_or(u64::MAX);
            if deleted < cutoff && std::fs::remove_dir_all(e.path()).is_ok() {
                n += 1;
            }
        }
    }
    n
}

pub fn search(cfg: &Config, vpath: &str, query: &str) -> FsResult<Value> {
    let r = resolve(cfg, vpath)?;
    let q = query.to_lowercase();
    if q.trim().is_empty() {
        return Ok(json!({ "results": [], "truncated": false }));
    }
    let root_real = r.root.path.canonicalize()?;
    let started = std::time::Instant::now();
    let mut results = vec![];
    let mut truncated = false;
    let walker = walkdir::WalkDir::new(&r.abs)
        .min_depth(1)
        .into_iter()
        .filter_entry(|e| !e.file_name().to_string_lossy().starts_with('.'));
    for (seen, e) in walker.flatten().enumerate() {
        if results.len() >= 300 || seen > 400_000 || started.elapsed() > Duration::from_secs(6) {
            truncated = true;
            break;
        }
        let name = e.file_name().to_string_lossy().into_owned();
        if !name.to_lowercase().contains(&q) {
            continue;
        }
        let Ok(rel) = e.path().strip_prefix(&r.root.path) else {
            continue;
        };
        if let Some(entry) = entry_for(&root_real, e.path(), name) {
            let mut v = serde_json::to_value(entry).unwrap_or_default();
            v["path"] = json!(join_vpath(&r.root.id, rel));
            results.push(v);
        }
    }
    Ok(json!({ "results": results, "truncated": truncated }))
}

pub fn read_text(cfg: &Config, vpath: &str) -> FsResult<Value> {
    let r = resolve(cfg, vpath)?;
    let meta = std::fs::metadata(&r.abs)?;
    if !meta.is_file() {
        return Err(FsError::bad("not a file"));
    }
    if meta.len() > TEXT_LIMIT {
        return Err(FsError(
            413,
            "file is too big to open as text (4 MiB max)".into(),
        ));
    }
    let bytes = std::fs::read(&r.abs)?;
    let text = String::from_utf8(bytes).map_err(|_| FsError(415, "not UTF-8 text".into()))?;
    Ok(
        json!({ "path": r.vpath(), "text": text, "mtime": secs(meta.modified()), "writable": r.root.writable }),
    )
}

/// Save text atomically. `expected_mtime` guards against overwriting a
/// change made elsewhere since the file was opened.
pub fn write_text(
    cfg: &Config,
    vpath: &str,
    text: &str,
    expected_mtime: Option<u64>,
) -> FsResult<u64> {
    use std::os::unix::fs::PermissionsExt;
    let r = resolve(cfg, vpath)?;
    r.writable()?;
    let meta = std::fs::metadata(&r.abs).ok();
    if let (Some(m), Some(exp)) = (&meta, expected_mtime)
        && secs(m.modified()) != exp
    {
        return Err(FsError(
            409,
            "the file changed on disk since you opened it".into(),
        ));
    }
    let dir = r.abs.parent().ok_or_else(|| FsError::bad("bad path"))?;
    let tmp = dir.join(format!(".holodrive-save-{}", crate::auth::random_hex(4)));
    std::fs::write(&tmp, text)?;
    if let Some(m) = &meta {
        std::fs::set_permissions(
            &tmp,
            std::fs::Permissions::from_mode(m.permissions().mode()),
        )?;
    }
    if let Err(e) = std::fs::rename(&tmp, &r.abs) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.into());
    }
    Ok(secs(std::fs::metadata(&r.abs)?.modified()))
}

/// Where an upload lands: `dir/rel` (rel may contain sub-folders from a
/// folder drop). Creates those sub-folders. Returns (final path, vpath).
pub fn upload_target(
    cfg: &Config,
    dir: &str,
    rel: &str,
    overwrite: bool,
) -> FsResult<(PathBuf, String)> {
    let d = resolve(cfg, dir)?;
    d.writable()?;
    let mut parts: Vec<&str> = rel.split('/').filter(|p| !p.is_empty()).collect();
    let name = parts
        .pop()
        .ok_or_else(|| FsError::bad("missing file name"))?;
    check_name(name)?;
    let mut sub = d.rel.clone();
    for p in &parts {
        check_name(p)?;
        sub.push(p);
    }
    let parent = d.root.path.join(&sub);
    std::fs::create_dir_all(&parent)?;
    contained(&d.root.path, &parent)?;
    let final_name = if overwrite {
        name.to_string()
    } else {
        unique_name(&parent, name)
    };
    let path = parent.join(&final_name);
    Ok((path, join_vpath(&d.root.id, &sub.join(final_name))))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(dir: &Path) -> Config {
        let mut c = Config::starter();
        c.roots = vec![
            Root {
                id: "a".into(),
                name: "A".into(),
                path: dir.join("a"),
                icon: "x".into(),
                writable: true,
            },
            Root {
                id: "ro".into(),
                name: "RO".into(),
                path: dir.join("ro"),
                icon: "x".into(),
                writable: false,
            },
        ];
        c
    }

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "holodrive-test-{tag}-{}",
            crate::auth::random_hex(4)
        ));
        std::fs::create_dir_all(d.join("a/sub")).unwrap();
        std::fs::create_dir_all(d.join("ro")).unwrap();
        std::fs::create_dir_all(d.join("outside")).unwrap();
        std::fs::write(d.join("a/sub/f.txt"), "hi").unwrap();
        std::fs::write(d.join("a/.secret"), "x").unwrap();
        std::fs::write(d.join("ro/r.txt"), "r").unwrap();
        std::os::unix::fs::symlink(d.join("outside"), d.join("a/escape")).unwrap();
        d
    }

    #[test]
    fn resolve_rules() {
        let d = scratch("resolve");
        let c = cfg(&d);
        assert!(resolve(&c, "a/sub/f.txt").is_ok());
        assert_eq!(resolve(&c, "a/../ro").err().unwrap().0, 400);
        assert_eq!(resolve(&c, "a/.secret").err().unwrap().0, 400);
        assert_eq!(resolve(&c, "a/escape").err().unwrap().0, 403);
        assert_eq!(resolve(&c, "nope/x").err().unwrap().0, 404);
        let l = list(&c, "a").unwrap();
        let names: Vec<&str> = l["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"sub") && !names.contains(&".secret"));
        let esc = l["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["name"] == "escape")
            .unwrap();
        assert_eq!(esc["escapes"], true);
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn read_only_root() {
        let d = scratch("ro");
        let c = cfg(&d);
        assert_eq!(mkdir(&c, "ro", "new").err().unwrap().0, 403);
        assert_eq!(trash(&c, &["ro/r.txt".into()]).err().unwrap().0, 403);
        assert_eq!(
            transfer(&c, &["ro/r.txt".into()], "a", false)
                .err()
                .unwrap()
                .0,
            403
        );
        assert!(transfer(&c, &["ro/r.txt".into()], "a", true).is_ok());
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn trash_and_restore() {
        let d = scratch("trash");
        let c = cfg(&d);
        let t = trash(&c, &["a/sub/f.txt".into()]).unwrap();
        assert!(!d.join("a/sub/f.txt").exists());
        assert_eq!(trash_list(&c).as_array().unwrap().len(), 1);
        let back = restore(&c, "a", t[0]["id"].as_str().unwrap()).unwrap();
        assert_eq!(back, "a/sub/f.txt");
        assert_eq!(
            std::fs::read_to_string(d.join("a/sub/f.txt")).unwrap(),
            "hi"
        );
        assert!(trash_list(&c).as_array().unwrap().is_empty());
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn names_and_moves() {
        let d = scratch("moves");
        let c = cfg(&d);
        assert_eq!(unique_name(&d.join("a/sub"), "f.txt"), "f (2).txt");
        assert_eq!(rename(&c, "a/sub/f.txt", "g.txt").unwrap(), "a/sub/g.txt");
        assert!(rename(&c, "a/sub/g.txt", "../x").is_err());
        assert!(transfer(&c, &["a/sub".into()], "a/sub", false).is_err());
        mkdir(&c, "a", "dest").unwrap();
        let moved = transfer(&c, &["a/sub/g.txt".into()], "a/dest", false).unwrap();
        assert_eq!(moved, vec!["a/dest/g.txt".to_string()]);
        let (p, v) = upload_target(&c, "a/dest", "deep/er/up.bin", false).unwrap();
        assert!(p.parent().unwrap().is_dir());
        assert_eq!(v, "a/dest/deep/er/up.bin");
        assert!(upload_target(&c, "a", "../../etc/passwd", false).is_err());
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn text_conflict() {
        let d = scratch("text");
        let c = cfg(&d);
        let t = read_text(&c, "a/sub/f.txt").unwrap();
        let m = t["mtime"].as_u64().unwrap();
        write_text(&c, "a/sub/f.txt", "new", Some(m)).unwrap();
        assert_eq!(
            write_text(&c, "a/sub/f.txt", "x", Some(m.saturating_sub(100)))
                .err()
                .unwrap()
                .0,
            409
        );
        std::fs::remove_dir_all(d).unwrap();
    }
}
