//! The Samba panel's server side. smb.conf is world-readable, so the share
//! list works without privileges; sessions, edits and service control go
//! through `holodrive-sambactl` via pkexec (see packaging/install-root.sh).

use crate::config::Config;
use crate::smbconf;
use serde_json::{Value, json};
use std::path::Path;
use std::process::{Command, Stdio};

pub const HELPER: &str = "/usr/local/bin/holodrive-sambactl";
const SMB_CONF: &str = "/etc/samba/smb.conf";

pub fn helper_installed() -> bool {
    std::fs::metadata(HELPER).is_ok_and(|m| m.is_file())
}

/// Run the helper; returns its `data` on success, its message on failure.
pub fn helper(args: &[&str], stdin: Option<String>) -> Result<(String, Value), String> {
    if !helper_installed() {
        return Err(format!(
            "{HELPER} isn't installed: run `sudo ./packaging/install-root.sh` in the holodrive checkout once"
        ));
    }
    let mut child = Command::new("pkexec")
        .arg(HELPER)
        .args(args)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("pkexec: {e}"))?;
    if let Some(input) = stdin {
        use std::io::Write;
        if let Some(mut s) = child.stdin.take() {
            s.write_all(input.as_bytes()).map_err(|e| e.to_string())?;
        }
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    let line = String::from_utf8_lossy(&out.stdout)
        .lines()
        .last()
        .unwrap_or("")
        .to_string();
    match serde_json::from_str::<Value>(&line) {
        Ok(v) if v["ok"] == true => Ok((
            v["message"].as_str().unwrap_or("").to_string(),
            v["data"].clone(),
        )),
        Ok(v) => Err(v["message"].as_str().unwrap_or("helper failed").to_string()),
        Err(_) => {
            let err = String::from_utf8_lossy(&out.stderr);
            if err.contains("must be setuid root") {
                Err("pkexec can't run inside a sandboxed service: reinstall holodrive.service from packaging/ (no PrivateTmp/ProtectSystem)".into())
            } else if out.status.code() == Some(126) || out.status.code() == Some(127) {
                Err(
                    "pkexec refused: the polkit rule isn't installed (run install-root.sh again)"
                        .into(),
                )
            } else {
                Err(format!(
                    "helper failed: {}",
                    err.lines().last().unwrap_or("no output").trim()
                ))
            }
        }
    }
}

/// Which Holodrive drive (and folder in it) a share path corresponds to.
fn drive_of(cfg: &Config, path: &str) -> Option<String> {
    let real = Path::new(path).canonicalize().ok()?;
    cfg.roots.iter().find_map(|r| {
        let root = r.path.canonicalize().ok()?;
        let rel = real.strip_prefix(&root).ok()?;
        Some(crate::fsops::join_vpath(&r.id, rel))
    })
}

pub fn overview(cfg: &Config) -> Value {
    let conf = std::fs::read_to_string(SMB_CONF);
    let mut shares: Vec<Value> = match &conf {
        Ok(text) => smbconf::shares(text)
            .into_iter()
            .map(|s| {
                json!({
                    "name": s.name, "path": s.path, "comment": s.comment, "read_only": s.read_only,
                    "browseable": s.browseable, "available": s.available,
                    "extra": s.extra.iter().map(|(k, v)| format!("{k} = {v}")).collect::<Vec<_>>(),
                    "drive": drive_of(cfg, &s.path), "exists": Path::new(&s.path).is_dir(),
                })
            })
            .collect(),
        Err(_) => vec![],
    };
    let mut status = Value::Null;
    let mut status_error = Value::Null;
    if helper_installed() {
        match helper(&["status"], None) {
            Ok((_, data)) => status = data,
            Err(e) => status_error = json!(e),
        }
    }
    // connected clients per share, from smbstatus tree connects
    if let Some(tcons) = status["smbstatus"]["tcons"].as_object() {
        for sh in &mut shares {
            let n = tcons
                .values()
                .filter(|t| {
                    t["service"]
                        .as_str()
                        .is_some_and(|s| s.eq_ignore_ascii_case(sh["name"].as_str().unwrap_or("")))
                })
                .count();
            sh["connections"] = json!(n);
        }
    }
    json!({
        "conf_readable": conf.is_ok(),
        "helper": helper_installed(),
        "service": status["service"],
        "smbstatus": status["smbstatus"],
        "status_error": status_error,
        "shares": shares,
        "host": std::fs::read_to_string("/etc/hostname").unwrap_or_default().trim(),
    })
}
