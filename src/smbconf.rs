//! Read and edit share sections of smb.conf, keeping everything else.
//!
//! Shared by Holodrive (read-only view) and `holodrive-sambactl` (the root
//! helper that writes). Only six things per share are ever written:
//! comment, path, read only, browseable, available and the section name.
//! Other per-share options, comments and the whole `[global]` section are
//! kept exactly as they are. Std-only on purpose (the helper is tiny).

pub const RESERVED: &[&str] = &["global", "homes", "printers", "print$", "ipc$"];

#[derive(Clone, Debug, PartialEq)]
pub struct Share {
    pub name: String,
    pub path: String,
    pub comment: String,
    pub read_only: bool,
    pub browseable: bool,
    pub available: bool,
    /// Other options set in the section, as written.
    pub extra: Vec<(String, String)>,
}

#[derive(Clone, Debug)]
pub struct Edit {
    pub name: String,
    pub path: String,
    pub comment: String,
    pub read_only: bool,
    pub browseable: bool,
    pub available: bool,
}

struct Section {
    name: String,
    /// Line index of the `[name]` header.
    header: usize,
    /// One past the last line that belongs to the section.
    end: usize,
}

fn norm_key(k: &str) -> String {
    k.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn parse_bool(v: &str) -> Option<bool> {
    match v.trim().to_lowercase().as_str() {
        "yes" | "true" | "1" | "on" => Some(true),
        "no" | "false" | "0" | "off" => Some(false),
        _ => None,
    }
}

fn header_name(line: &str) -> Option<String> {
    let t = line.trim();
    (t.starts_with('[') && t.ends_with(']') && t.len() > 2)
        .then(|| t[1..t.len() - 1].trim().to_string())
}

fn entry(line: &str) -> Option<(String, String)> {
    let t = line.trim();
    if t.is_empty() || t.starts_with('#') || t.starts_with(';') {
        return None;
    }
    let (k, v) = t.split_once('=')?;
    Some((norm_key(k), v.trim().to_string()))
}

fn sections(lines: &[&str]) -> Vec<Section> {
    let mut out: Vec<Section> = vec![];
    for (i, l) in lines.iter().enumerate() {
        if let Some(name) = header_name(l) {
            if let Some(last) = out.last_mut() {
                last.end = i;
            }
            out.push(Section {
                name,
                header: i,
                end: lines.len(),
            });
        }
    }
    // Trailing blank/comment lines before the next header belong to the
    // gap, not the section: trim them so edits don't eat them.
    for s in &mut out {
        while s.end > s.header + 1 && entry(lines[s.end - 1]).is_none() {
            s.end -= 1;
        }
    }
    out
}

/// Defaults from `[global]` that shares inherit, on top of Samba's own.
fn global_defaults(lines: &[&str], secs: &[Section]) -> (bool, bool, bool) {
    let (mut ro, mut br, mut av) = (true, true, true);
    if let Some(g) = secs.iter().find(|s| s.name.eq_ignore_ascii_case("global")) {
        for l in &lines[g.header + 1..g.end] {
            if let Some((k, v)) = entry(l) {
                apply_flag(&k, &v, &mut ro, &mut br, &mut av);
            }
        }
    }
    (ro, br, av)
}

fn apply_flag(k: &str, v: &str, ro: &mut bool, br: &mut bool, av: &mut bool) -> bool {
    let Some(b) = parse_bool(v) else { return false };
    match k {
        "read only" => *ro = b,
        "writable" | "writeable" | "write ok" => *ro = !b,
        "browseable" | "browsable" => *br = b,
        "available" => *av = b,
        _ => return false,
    }
    true
}

pub fn shares(text: &str) -> Vec<Share> {
    let lines: Vec<&str> = text.lines().collect();
    let secs = sections(&lines);
    let (gro, gbr, gav) = global_defaults(&lines, &secs);
    secs.iter()
        .filter(|s| !RESERVED.iter().any(|r| s.name.eq_ignore_ascii_case(r)))
        .map(|s| {
            let mut sh = Share {
                name: s.name.clone(),
                path: String::new(),
                comment: String::new(),
                read_only: gro,
                browseable: gbr,
                available: gav,
                extra: vec![],
            };
            for l in &lines[s.header + 1..s.end] {
                let Some((k, v)) = entry(l) else { continue };
                match k.as_str() {
                    "path" => sh.path = v,
                    "comment" => sh.comment = v,
                    _ => {
                        if !apply_flag(
                            &k,
                            &v,
                            &mut sh.read_only,
                            &mut sh.browseable,
                            &mut sh.available,
                        ) {
                            sh.extra.push((k, v));
                        }
                    }
                }
            }
            sh
        })
        .collect()
}

pub fn valid_share_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.len() > 40 {
        return Err("share name must be 1–40 characters".into());
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err("share name: letters, digits, - and _ only".into());
    }
    if RESERVED.iter().any(|r| name.eq_ignore_ascii_case(r)) {
        return Err(format!("{name:?} is reserved by Samba"));
    }
    Ok(())
}

fn valid_value(what: &str, v: &str, max: usize) -> Result<(), String> {
    if v.len() > max || v.chars().any(|c| c.is_control()) || v.contains('[') || v.contains(']') {
        return Err(format!(
            "{what}: no brackets or control characters, at most {max} characters"
        ));
    }
    Ok(())
}

const MANAGED: &[&str] = &[
    "comment",
    "path",
    "read only",
    "writable",
    "writeable",
    "write ok",
    "browseable",
    "browsable",
    "available",
];

fn managed_lines(e: &Edit) -> Vec<String> {
    let yn = |b: bool| if b { "yes" } else { "no" };
    let mut v = vec![];
    if !e.comment.is_empty() {
        v.push(format!("   comment = {}", e.comment));
    }
    v.push(format!("   path = {}", e.path));
    v.push(format!("   read only = {}", yn(e.read_only)));
    if !e.browseable {
        v.push("   browseable = no".into());
    }
    if !e.available {
        v.push("   available = no".into());
    }
    v
}

/// Add a share, or update `original` (which may be renamed to `e.name`).
pub fn upsert(text: &str, e: &Edit, original: Option<&str>) -> Result<String, String> {
    valid_share_name(&e.name)?;
    valid_value("comment", &e.comment, 200)?;
    valid_value("path", &e.path, 1024)?;
    if !e.path.starts_with('/') {
        return Err("path must be absolute".into());
    }
    let lines: Vec<&str> = text.lines().collect();
    let secs = sections(&lines);
    let find = |n: &str| secs.iter().find(|s| s.name.eq_ignore_ascii_case(n));
    let target = original.unwrap_or(&e.name);
    let clash = find(&e.name);
    if let Some(c) = clash
        && !c.name.eq_ignore_ascii_case(target)
    {
        return Err(format!("a share named {:?} already exists", c.name));
    }
    let mut out: Vec<String> = vec![];
    match find(target) {
        Some(s) if original.is_some() || clash.is_some() => {
            out.extend(lines[..s.header].iter().map(|l| l.to_string()));
            out.push(format!("[{}]", e.name));
            out.extend(managed_lines(e));
            for l in &lines[s.header + 1..s.end] {
                let keep = entry(l).is_none_or(|(k, _)| !MANAGED.contains(&k.as_str()));
                if keep && !l.trim().is_empty() {
                    out.push(l.to_string());
                }
            }
            out.extend(lines[s.end..].iter().map(|l| l.to_string()));
        }
        None if original.is_some() => return Err(format!("no share named {target:?}")),
        _ => {
            out.extend(lines.iter().map(|l| l.to_string()));
            while out.last().is_some_and(|l| l.trim().is_empty()) {
                out.pop();
            }
            out.push(String::new());
            out.push(format!("[{}]", e.name));
            out.extend(managed_lines(e));
        }
    }
    Ok(out.join("\n") + "\n")
}

pub fn remove(text: &str, name: &str) -> Result<String, String> {
    valid_share_name(name)?;
    let lines: Vec<&str> = text.lines().collect();
    let secs = sections(&lines);
    let s = secs
        .iter()
        .find(|s| s.name.eq_ignore_ascii_case(name))
        .ok_or_else(|| format!("no share named {name:?}"))?;
    let mut out: Vec<String> = lines[..s.header].iter().map(|l| l.to_string()).collect();
    // A comment block written right above the header describes this share.
    while out
        .last()
        .is_some_and(|l| l.trim_start().starts_with('#') || l.trim_start().starts_with(';'))
    {
        out.pop();
    }
    while out.last().is_some_and(|l| l.trim().is_empty()) {
        out.pop();
    }
    let rest: Vec<String> = lines[s.end..].iter().map(|l| l.to_string()).collect();
    if !rest.iter().all(|l| l.trim().is_empty()) {
        out.push(String::new());
        out.extend(rest.into_iter().skip_while(|l| l.trim().is_empty()));
    }
    Ok(out.join("\n") + "\n")
}

/// Parameter-name fragments that run programs, read other files or
/// change where Samba gets its config. Matched against the normalized key.
const FORBIDDEN_FRAGMENTS: &[&str] = &[
    "command",
    "script",
    "exec",
    "program",
    "include",
    "config file",
    "config backend",
    "magic",
    "panic action",
    "dfree",
    "quota",
    "message command",
    "passdb backend",
    "smb passwd file",
    "username map",
    "registry shares",
    "usershare path",
];
/// Whole parameter names refused (access escalation or guest access).
const FORBIDDEN_KEYS: &[&str] = &[
    "force user",
    "force group",
    "admin users",
    "guest ok",
    "public",
    "guest only",
    "guest account",
    "root directory",
    "root dir",
    "root",
    "allow insecure wide links",
];
/// Path parameters that must stay under Samba's own directories.
const PATH_KEYS: &[(&str, &[&str])] = &[
    ("log file", &["/var/log/samba/"]),
    (
        "lock directory",
        &["/var/lib/samba", "/run/samba", "/var/cache/samba"],
    ),
    ("state directory", &["/var/lib/samba"]),
    ("cache directory", &["/var/cache/samba", "/var/lib/samba"]),
    ("pid directory", &["/run/samba", "/run"]),
    ("private dir", &["/var/lib/samba/private"]),
    ("private directory", &["/var/lib/samba/private"]),
    ("ncalrpc dir", &["/run/samba", "/var/lib/samba"]),
    ("binddns dir", &["/var/lib/samba"]),
];

/// Every (section, key, value, 1-based line) in the file.
pub fn entries(text: &str) -> Vec<(String, String, String, usize)> {
    let mut section = String::new();
    let mut out = vec![];
    for (i, l) in text.lines().enumerate() {
        if let Some(n) = header_name(l) {
            section = n;
        } else if let Some((k, v)) = entry(l) {
            out.push((section.clone(), k, v, i + 1));
        }
    }
    out
}

/// The safety rails for a whole smb.conf written from the web. Returns the
/// warnings (allowed, but worth a confirm) or the errors (refused).
pub fn check_rails(text: &str) -> Result<Vec<String>, Vec<String>> {
    let mut errors = vec![];
    let mut warnings = vec![];
    if text.len() > 256 * 1024 {
        return Err(vec!["smb.conf is limited to 256 KiB here".into()]);
    }
    for (i, l) in text.lines().enumerate() {
        let t = l.trim_end();
        if t.ends_with('\\') && !t.trim_start().starts_with('#') && !t.trim_start().starts_with(';')
        {
            errors.push(format!(
                "line {}: line continuations (trailing \\) aren't allowed here",
                i + 1
            ));
        }
        if l.contains('\0') {
            errors.push(format!("line {}: NUL byte", i + 1));
        }
        let tt = l.trim();
        if tt.starts_with('[') && !tt.ends_with(']') {
            errors.push(format!("line {}: unclosed section header", i + 1));
        }
    }
    let mut seen_global = false;
    for name in text.lines().filter_map(header_name) {
        let low = name.to_lowercase();
        if low == "global" {
            seen_global = true;
        } else if low == "homes" || low == "printers" || low == "print$" {
            errors.push(format!("[{name}] sections aren't allowed here"));
        } else if let Err(e) = valid_share_name(&name) {
            errors.push(format!("[{name}]: {e}"));
        }
    }
    if !seen_global {
        errors.push("the [global] section is missing".into());
    }
    let mut encrypt = None;
    let mut min_proto = None;
    let mut hosts_allow = false;
    for (sec, k, v, line) in entries(text) {
        let where_ = format!("line {line} [{sec}] {k}");
        if FORBIDDEN_KEYS.contains(&k.as_str()) || FORBIDDEN_FRAGMENTS.iter().any(|f| k.contains(f))
        {
            errors.push(format!("{where_}: not allowed from Holodrive (it could run programs, read other files or widen access)"));
            continue;
        }
        let lv = v.to_lowercase();
        match k.as_str() {
            "map to guest" if lv != "never" => {
                errors.push(format!("{where_}: only \"never\" is allowed"))
            }
            "printcap name" | "printcap" if v != "/dev/null" => {
                errors.push(format!("{where_}: only /dev/null is allowed"))
            }
            "wide links" if parse_bool(&v) == Some(true) => errors.push(format!(
                "{where_}: following links outside shares isn't allowed"
            )),
            "path" if sec.eq_ignore_ascii_case("global") => {
                errors.push(format!("{where_}: path belongs in a share"))
            }
            _ => {}
        }
        if let Some((_, allowed)) = PATH_KEYS.iter().find(|(pk, _)| *pk == k) {
            let clean = !v.contains("..");
            if !clean || !allowed.iter().any(|a| v.starts_with(a)) {
                errors.push(format!(
                    "{where_}: must stay under {}",
                    allowed.join(" or ")
                ));
            }
        }
        if sec.eq_ignore_ascii_case("global") {
            match k.as_str() {
                "server smb encrypt" | "smb encrypt" => encrypt = Some(lv.clone()),
                "server min protocol" | "min protocol" => min_proto = Some(v.to_uppercase()),
                "hosts allow" | "allow hosts" => hosts_allow = true,
                _ => {}
            }
        }
    }
    if !matches!(encrypt.as_deref(), Some("required" | "mandatory" | "force")) {
        warnings.push("encryption isn't required ([global] server smb encrypt = required)".into());
    }
    if !min_proto.as_deref().is_some_and(|p| p.starts_with("SMB3")) {
        warnings.push("clients older than SMB3 are allowed ([global] server min protocol)".into());
    }
    if !hosts_allow {
        warnings.push(
            "no [global] hosts allow: any address that reaches port 445 may try to log in".into(),
        );
    }
    if errors.is_empty() {
        Ok(warnings)
    } else {
        Err(errors)
    }
}

/// A short content hash (FNV-1a 64): the editor sends back the one it
/// opened, so a stale tab can't overwrite newer changes.
pub fn content_hash(text: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in text.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

/// (section, path) for every share that sets one.
pub fn share_paths(text: &str) -> Vec<(String, String)> {
    entries(text)
        .into_iter()
        .filter(|(sec, k, _, _)| k == "path" && !sec.eq_ignore_ascii_case("global"))
        .map(|(sec, _, v, _)| (sec, v))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONF: &str = "# top comment
[global]
   server role = standalone server
   read only = no
   valid users = raven

[Shared]
   comment = Common folder
   path = /home/raven/Shared
   veto files = /.DS_Store/

# isos below
[ISOs]
   path = /home/raven/ISOs
   read only = yes
";

    fn edit(name: &str, path: &str) -> Edit {
        Edit {
            name: name.into(),
            path: path.into(),
            comment: String::new(),
            read_only: false,
            browseable: true,
            available: true,
        }
    }

    #[test]
    fn reads_shares_with_global_defaults() {
        let s = shares(CONF);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].name, "Shared");
        assert!(!s[0].read_only, "inherits read only = no from [global]");
        assert_eq!(
            s[0].extra,
            vec![("veto files".to_string(), "/.DS_Store/".to_string())]
        );
        assert!(s[1].read_only);
    }

    #[test]
    fn add_keeps_everything_else() {
        let out = upsert(CONF, &edit("Music", "/home/raven/Music"), None).unwrap();
        assert!(out.starts_with("# top comment\n[global]"));
        assert!(out.contains("# isos below"));
        assert!(out.ends_with("[Music]\n   path = /home/raven/Music\n   read only = no\n"));
        assert_eq!(shares(&out).len(), 3);
    }

    #[test]
    fn update_and_rename_keep_extra_options() {
        let mut e = edit("Common", "/home/raven/Shared");
        e.read_only = true;
        e.available = false;
        e.comment = "both boxes".into();
        let out = upsert(CONF, &e, Some("Shared")).unwrap();
        let s = shares(&out);
        let c = s.iter().find(|x| x.name == "Common").unwrap();
        assert!(c.read_only && !c.available);
        assert_eq!(c.comment, "both boxes");
        assert_eq!(c.extra.len(), 1, "veto files kept");
        assert!(out.contains("# isos below"), "gap comment kept");
        assert!(!out.contains("[Shared]"));
        assert!(out.contains("server role = standalone server"));
    }

    #[test]
    fn refuses_bad_input() {
        assert!(upsert(CONF, &edit("global", "/x"), None).is_err());
        assert!(upsert(CONF, &edit("bad name", "/x"), None).is_err());
        assert!(
            upsert(CONF, &edit("ISOs", "/x"), None).is_ok(),
            "same name updates"
        );
        assert!(
            upsert(CONF, &edit("isos", "/x"), Some("Shared")).is_err(),
            "rename onto another share"
        );
        assert!(upsert(CONF, &edit("New", "relative"), None).is_err());
        let mut e = edit("New", "/x");
        e.comment = "x\n[global]\nguest ok = yes".into();
        assert!(
            upsert(CONF, &e, None).is_err(),
            "no injection through comment"
        );
        assert!(remove(CONF, "nope").is_err());
    }

    #[test]
    fn remove_share() {
        let out = remove(CONF, "Shared").unwrap();
        assert!(!out.contains("[Shared]") && !out.contains("veto files"));
        assert!(out.contains("[global]") && out.contains("[ISOs]") && out.contains("# isos below"));
        assert_eq!(shares(&out).len(), 1);
        let out2 = remove(CONF, "ISOs").unwrap();
        assert!(
            out2.trim_end().ends_with("veto files = /.DS_Store/"),
            "its comment goes with it"
        );
    }

    const SAFE: &str = "[global]
   server smb encrypt = required
   server min protocol = SMB3_11
   hosts allow = 192.168.88.0/24
   map to guest = never
   log file = /var/log/samba/%m.log
   printcap name = /dev/null

[Music]
   path = /home/raven/Music
";

    #[test]
    fn rails_accept_a_safe_file() {
        assert_eq!(check_rails(SAFE), Ok(vec![]));
        assert_eq!(
            share_paths(SAFE),
            vec![("Music".to_string(), "/home/raven/Music".to_string())]
        );
    }

    #[test]
    fn rails_refuse_dangerous_settings() {
        for bad in [
            "   root preexec = /bin/sh -c id",
            "   preexec = touch /tmp/x",
            "   ROOT   PREEXEC = x",
            "   include = /etc/shadow",
            "   force user = root",
            "   admin users = raven",
            "   guest ok = yes",
            "   public = yes",
            "   username map = /tmp/map",
            "   wide links = yes",
            "   print command = sh",
            "   printcap name = /etc/shadow",
            "   add share command = sh",
            "   log file = /etc/cron.d/x",
            "   log file = /var/log/samba/../../etc/x",
            "   map to guest = bad user",
            "   config file = /tmp/evil.conf",
            "   magic script = run.sh",
            "   path = \\\n/etc",
        ] {
            let t = format!("{SAFE}{bad}\n");
            assert!(check_rails(&t).is_err(), "should refuse: {bad}");
        }
        assert!(check_rails(&format!("{SAFE}[homes]\n   read only = no\n")).is_err());
        assert!(
            check_rails("[Music]\n   path = /x\n").is_err(),
            "no [global]"
        );
        assert!(check_rails(&format!("{SAFE}[bad name]\n")).is_err());
    }

    #[test]
    fn rails_warn_on_weaker_global() {
        let weak = SAFE
            .replace("required", "desired")
            .replace("SMB3_11", "SMB2");
        let w = check_rails(&weak).unwrap();
        assert_eq!(w.len(), 2, "{w:?}");
    }
}
