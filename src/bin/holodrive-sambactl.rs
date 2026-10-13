//! holodrive-sambactl — Holodrive's root helper for Samba.
//!
//! Installed root-owned at /usr/local/bin and run through `pkexec`; a polkit
//! rule lets the one user who installed Holodrive run it without a password
//! (Holodrive runs as a service, so there's no agent to ask). Because of
//! that it trusts nothing it's given:
//!
//! - the user is taken from `PKEXEC_UID` (or `SUDO_UID`), never arguments;
//! - it edits share sections only (see `smbconf.rs`): name, path, comment,
//!   read only, browseable, available. `[global]` is never touched;
//! - a share's path must be an existing folder inside that user's home,
//!   with no hidden component, not the home itself, and not under anything
//!   listed in /etc/holodrive/samba-deny.list;
//! - every new smb.conf is checked with `testparm` before it replaces the
//!   old one, which is backed up to /var/lib/holodrive/smb-backups first;
//! - every change is logged to the journal (`logger -t holodrive-sambactl`).
//!
//! Usage: status | upsert (JSON on stdin) | remove <share> | reload |
//!        restart | close-share <share> | backups | restore <backup>
//! Prints one JSON line: {"ok": bool, "message": "...", "data": ...}.

#[path = "../smbconf.rs"]
mod smbconf;

use serde_json::{Value, json};
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const SMB_CONF: &str = "/etc/samba/smb.conf";
const BACKUP_DIR: &str = "/var/lib/holodrive/smb-backups";
const DENY_LIST: &str = "/etc/holodrive/samba-deny.list";
const LOCK: &str = "/run/holodrive-sambactl.lock";
const KEEP_BACKUPS: usize = 20;

struct Caller {
    user: String,
    home: PathBuf,
}

fn main() {
    let (ok, message, data) = match run() {
        Ok((m, d)) => (true, m, d),
        Err(m) => (false, m, Value::Null),
    };
    println!("{}", json!({ "ok": ok, "message": message, "data": data }));
    std::process::exit(if ok { 0 } else { 1 });
}

type Out = Result<(String, Value), String>;

fn run() -> Out {
    if unsafe { libc::geteuid() } != 0 {
        return Err("holodrive-sambactl must run as root (via pkexec)".into());
    }
    let caller = caller()?;
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg = |i: usize| {
        args.get(i)
            .cloned()
            .ok_or_else(|| "missing argument".to_string())
    };
    match args.first().map(String::as_str).unwrap_or("") {
        "status" => status(),
        "upsert" => locked(|| upsert(&caller)),
        "remove" => {
            let name = arg(1)?;
            locked(|| change(&caller, &format!("removed share [{name}]"), |t| smbconf::remove(t, &name)))
        }
        "reload" => smbcontrol(&["smbd", "reload-config"]).map(|_| ("config reloaded".into(), Value::Null)),
        "restart" => {
            run_cmd("/usr/bin/systemctl", &["restart", "smb.service"])?;
            log(&caller, "restarted smb.service");
            Ok(("smbd restarted".into(), Value::Null))
        }
        "close-share" => {
            let name = arg(1)?;
            smbconf::valid_share_name(&name)?;
            smbcontrol(&["smbd", "close-share", &name])?;
            log(&caller, &format!("disconnected everyone from [{name}]"));
            Ok((format!("disconnected everyone from {name}"), Value::Null))
        }
        "backups" => backups().map(|b| ("backups".into(), b)),
        "restore" => {
            let name = arg(1)?;
            locked(|| restore(&caller, &name))
        }
        _ => Err("usage: status | upsert | remove <share> | reload | restart | close-share <share> | backups | restore <backup>".into()),
    }
}

fn caller() -> Result<Caller, String> {
    let uid: u32 = std::env::var("PKEXEC_UID")
        .or_else(|_| std::env::var("SUDO_UID"))
        .map_err(|_| "run through pkexec or sudo".to_string())?
        .parse()
        .map_err(|_| "bad caller uid".to_string())?;
    if uid == 0 {
        return Err("refusing to act for root".into());
    }
    let pw = unsafe { libc::getpwuid(uid) };
    if pw.is_null() {
        return Err(format!("no user with uid {uid}"));
    }
    let cstr = |p: *const libc::c_char| {
        unsafe { std::ffi::CStr::from_ptr(p) }
            .to_string_lossy()
            .into_owned()
    };
    let (user, home) = unsafe { (cstr((*pw).pw_name), cstr((*pw).pw_dir)) };
    Ok(Caller {
        user,
        home: PathBuf::from(home),
    })
}

fn log(c: &Caller, what: &str) {
    let _ = Command::new("/usr/bin/logger")
        .args([
            "-t",
            "holodrive-sambactl",
            &format!("{} (for {})", what, c.user),
        ])
        .status();
}

fn run_cmd(bin: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new(bin)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("{bin}: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        Err(format!(
            "{bin} failed: {}",
            err.lines().last().unwrap_or("").trim()
        ))
    }
}

fn smbcontrol(args: &[&str]) -> Result<String, String> {
    run_cmd("/usr/bin/smbcontrol", args)
}

/// One change at a time (a second Holodrive tab, a double click…).
fn locked(f: impl FnOnce() -> Out) -> Out {
    use std::os::unix::io::AsRawFd;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(LOCK)
        .map_err(|e| format!("lock: {e}"))?;
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err("couldn't take the lock".into());
    }
    f()
}

fn status() -> Out {
    let conf = std::fs::read_to_string(SMB_CONF).map_err(|e| format!("{SMB_CONF}: {e}"))?;
    let smbstatus: Value = Command::new("/usr/bin/smbstatus")
        .args(["--json"])
        .stdin(Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| serde_json::from_slice(&o.stdout).ok())
        .unwrap_or(Value::Null);
    let active = Command::new("/usr/bin/systemctl")
        .args(["is-active", "smb.service"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    let shares: Vec<Value> = smbconf::shares(&conf)
        .into_iter()
        .map(|s| {
            json!({ "name": s.name, "path": s.path, "comment": s.comment, "read_only": s.read_only,
                         "browseable": s.browseable, "available": s.available })
        })
        .collect();
    Ok((
        "status".into(),
        json!({ "service": active, "smbstatus": smbstatus, "shares": shares }),
    ))
}

fn deny_list(c: &Caller) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_to_string(DENY_LIST)
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| PathBuf::from(l.replace("$HOME", &c.home.to_string_lossy())))
        .collect();
    v.push(c.home.join(".ssh"));
    v.push(c.home.join(".gnupg"));
    v
}

/// The folder must exist, be inside the caller's home (after symlinks),
/// not be the home itself, have no hidden component and not be denied.
fn check_path(c: &Caller, path: &str) -> Result<String, String> {
    let p = Path::new(path);
    let real = p.canonicalize().map_err(|e| format!("{path}: {e}"))?;
    if !real.is_dir() {
        return Err(format!("{path} isn't a folder"));
    }
    let home = c.home.canonicalize().map_err(|e| e.to_string())?;
    let rel = real
        .strip_prefix(&home)
        .map_err(|_| format!("only folders inside {} can be shared", home.display()))?;
    if rel.as_os_str().is_empty() {
        return Err("sharing your whole home folder isn't allowed".into());
    }
    if rel
        .components()
        .any(|c| c.as_os_str().to_string_lossy().starts_with('.'))
    {
        return Err("hidden folders (starting with .) can't be shared".into());
    }
    for d in deny_list(c) {
        let d = d.canonicalize().unwrap_or(d);
        if real.starts_with(&d) || d.starts_with(&real) {
            return Err(format!(
                "{} is on the private list ({DENY_LIST})",
                d.display()
            ));
        }
    }
    let mode = std::fs::metadata(&real)
        .map_err(|e| e.to_string())?
        .permissions()
        .mode();
    if mode & 0o002 != 0 {
        return Err("world-writable folders can't be shared".into());
    }
    Ok(real.to_string_lossy().into_owned())
}

fn upsert(c: &Caller) -> Out {
    let mut input = String::new();
    std::io::stdin()
        .take(64 * 1024)
        .read_to_string(&mut input)
        .map_err(|e| e.to_string())?;
    let v: Value = serde_json::from_str(&input).map_err(|e| format!("bad JSON: {e}"))?;
    let s = |k: &str| v[k].as_str().unwrap_or("").to_string();
    let b = |k: &str, d: bool| v[k].as_bool().unwrap_or(d);
    let path = check_path(c, &s("path"))?;
    let edit = smbconf::Edit {
        name: s("name"),
        path,
        comment: s("comment"),
        read_only: b("read_only", false),
        browseable: b("browseable", true),
        available: b("available", true),
    };
    let original = v["original"]
        .as_str()
        .filter(|o| !o.is_empty())
        .map(str::to_string);
    if let Some(o) = &original {
        smbconf::valid_share_name(o)?;
    }
    let what = match &original {
        Some(o) if !o.eq_ignore_ascii_case(&edit.name) => {
            format!("renamed share [{o}] to [{}] ({})", edit.name, edit.path)
        }
        Some(_) => format!("updated share [{}] ({})", edit.name, edit.path),
        None => format!("added share [{}] ({})", edit.name, edit.path),
    };
    change(c, &what, |t| smbconf::upsert(t, &edit, original.as_deref()))
}

/// Apply `f` to smb.conf: validate with testparm, back up, install, reload.
fn change(c: &Caller, what: &str, f: impl FnOnce(&str) -> Result<String, String>) -> Out {
    let current = std::fs::read_to_string(SMB_CONF).map_err(|e| format!("{SMB_CONF}: {e}"))?;
    let next = f(&current)?;
    if next == current {
        return Ok(("nothing to change".into(), Value::Null));
    }
    let candidate = PathBuf::from(format!("{SMB_CONF}.holodrive-new"));
    std::fs::write(&candidate, &next).map_err(|e| e.to_string())?;
    std::fs::set_permissions(&candidate, std::fs::Permissions::from_mode(0o644))
        .map_err(|e| e.to_string())?;
    if let Err(e) = run_cmd(
        "/usr/bin/testparm",
        &["-s", "--suppress-prompt", &candidate.to_string_lossy()],
    ) {
        let _ = std::fs::remove_file(&candidate);
        return Err(format!(
            "testparm rejected the new config, nothing changed: {e}"
        ));
    }
    let backup = backup(&current)?;
    std::fs::rename(&candidate, SMB_CONF).map_err(|e| e.to_string())?;
    let reload = smbcontrol(&["smbd", "reload-config"]);
    log(c, &format!("{what}; backup {backup}"));
    let msg = match reload {
        Ok(_) => what.to_string(),
        Err(_) => format!("{what} (saved; smbd isn't running, so nothing to reload)"),
    };
    Ok((msg, json!({ "backup": backup })))
}

fn backup(text: &str) -> Result<String, String> {
    std::fs::create_dir_all(BACKUP_DIR).map_err(|e| e.to_string())?;
    // Seconds plus nanoseconds: two changes in the same second must not
    // overwrite each other's backup (the older one may be the original).
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let name = format!("smb.conf.{}-{:09}", now.as_secs(), now.subsec_nanos());
    std::fs::write(Path::new(BACKUP_DIR).join(&name), text).map_err(|e| e.to_string())?;
    let mut all: Vec<_> = std::fs::read_dir(BACKUP_DIR)
        .map_err(|e| e.to_string())?
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("smb.conf."))
        .collect();
    all.sort();
    while all.len() > KEEP_BACKUPS {
        let _ = std::fs::remove_file(Path::new(BACKUP_DIR).join(all.remove(0)));
    }
    Ok(name)
}

fn backups() -> Result<Value, String> {
    let mut all: Vec<Value> = std::fs::read_dir(BACKUP_DIR)
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| {
                    let n = e.file_name().to_string_lossy().into_owned();
                    let secs: u64 = n
                        .strip_prefix("smb.conf.")?
                        .split('-')
                        .next()?
                        .parse()
                        .ok()?;
                    let text = std::fs::read_to_string(e.path()).unwrap_or_default();
                    let shares: Vec<String> =
                        smbconf::shares(&text).into_iter().map(|s| s.name).collect();
                    Some(json!({ "name": n, "time": secs, "shares": shares }))
                })
                .collect()
        })
        .unwrap_or_default();
    all.sort_by(|a, b| b["name"].as_str().cmp(&a["name"].as_str()));
    Ok(json!(all))
}

fn restore(c: &Caller, name: &str) -> Out {
    let ok_name = name.strip_prefix("smb.conf.").is_some_and(|s| {
        !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit() || b == b'-') && !s.starts_with('-')
    });
    if !ok_name {
        return Err("bad backup name".into());
    }
    let text = std::fs::read_to_string(Path::new(BACKUP_DIR).join(name))
        .map_err(|e| format!("{name}: {e}"))?;
    change(c, &format!("restored {name}"), |_| Ok(text))
}
