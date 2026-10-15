//! HTTP routes and the request guard.
//!
//! Holodrive listens on 127.0.0.1 and is published to the tailnet by
//! `tailscale serve`. The guard:
//! - accepts only known `Host` values (blocks DNS rebinding);
//! - requires a session cookie for every `/api/` call except login,
//!   session and the read-only theme endpoints;
//! - requires, on every state-changing request, the session's CSRF token
//!   in `X-Holodrive-Csrf` and (when sent) an `Origin` on an allowed host.
//!
//! Raw files are served with `Content-Security-Policy: sandbox`, so an
//! uploaded HTML or SVG file can't run script in Holodrive's origin.

use crate::auth::{self, Auth, LoginBrake};
use crate::config::Config;
use crate::fsops::{self, FsError};
use crate::{cybergrid, media, samba, smbconf, snapshots, uploads, views};
use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Path, Query, Request, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tower::ServiceExt;

pub struct AppState {
    pub cfg: Arc<Config>,
    pub auth: Auth,
    pub port: u16,
    pub thumbs: tokio::sync::Semaphore,
    /// Flips to true when the server starts shutting down, so endless
    /// streams (theme events) end and graceful shutdown can finish.
    pub stopping: tokio::sync::watch::Receiver<bool>,
    /// Resumable uploads with a chunk being written right now.
    pub sending: std::sync::Mutex<std::collections::HashSet<String>>,
}

type Shared = Arc<AppState>;

pub fn router(state: Shared) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/static/:file", get(asset))
        .route("/static/fonts/:file", get(font))
        .route("/vendor/tokens.css", get(tokens_css))
        .route("/api/cybergrid/themes", get(cybergrid::list_themes))
        .route("/api/cybergrid/events", get(cybergrid::theme_events))
        .route("/api/cybergrid/css/:name", get(cybergrid::theme_css))
        .route("/api/session", get(session))
        .route("/api/login", post(login))
        .route("/api/logout", post(logout))
        .route("/api/roots", get(roots))
        .route("/api/list", get(list))
        .route("/api/stat", get(stat))
        .route("/api/exists", get(exists))
        .route("/api/search", get(search))
        .route("/api/text", get(text_get).post(text_put))
        .route("/api/raw", get(raw))
        .route("/api/thumb", get(thumb))
        .route("/api/zip", get(zip))
        .route("/api/mkdir", post(mkdir))
        .route("/api/newfile", post(newfile))
        .route("/api/rename", post(rename))
        .route("/api/transfer", post(transfer))
        .route("/api/trash", get(trash_list).post(trash))
        .route("/api/trash/restore", post(restore))
        .route("/api/trash/purge", post(purge))
        .route("/api/snap/timeline", get(snap_timeline))
        .route("/api/snap/list", get(snap_list))
        .route("/api/snap/versions", get(snap_versions))
        .route("/api/snap/raw", get(snap_raw))
        .route("/api/snap/thumb", get(snap_thumb))
        .route("/api/snap/text", get(snap_text))
        .route("/api/snap/restore", post(snap_restore))
        .route("/api/samba", get(samba_overview))
        .route("/api/samba/share", post(samba_share))
        .route("/api/samba/remove", post(samba_remove))
        .route("/api/samba/action", post(samba_action))
        .route("/api/samba/backups", get(samba_backups))
        .route("/api/samba/conf", get(samba_conf).post(samba_conf_write))
        .route("/api/samba/restore", post(samba_restore))
        .route(
            "/api/upload",
            post(upload).layer(DefaultBodyLimit::disable()),
        )
        .route("/api/upload/start", post(upload_start))
        .route("/api/upload/status", get(upload_status))
        .route(
            "/api/upload/chunk",
            put(upload_chunk).layer(DefaultBodyLimit::disable()),
        )
        .route("/api/upload/finish", post(upload_finish))
        .route("/api/upload/cancel", post(upload_cancel))
        .layer(middleware::from_fn_with_state(state.clone(), guard))
        .with_state(state)
}

fn allowed_hosts(s: &AppState) -> Vec<String> {
    let mut v = vec![
        format!("127.0.0.1:{}", s.port),
        format!("localhost:{}", s.port),
    ];
    v.extend(s.cfg.hosts.iter().cloned());
    v
}

fn is_local_host(host: &str, port: u16) -> bool {
    host == format!("127.0.0.1:{port}") || host == format!("localhost:{port}")
}

/// Direct browser access on this machine, with the bypass enabled. Anything
/// that came through a proxy (tailscale serve adds these headers) never counts,
/// even if it claims a localhost Host.
fn local_trusted(s: &AppState, req: &Request, host: &str) -> bool {
    const PROXIED: [&str; 3] = ["x-forwarded-for", "forwarded", "tailscale-user-login"];
    s.cfg.local_auth_bypass
        && is_local_host(host, s.port)
        && !PROXIED.iter().any(|h| req.headers().contains_key(*h))
}

fn session_id(req: &Request) -> Option<String> {
    let cookies = req.headers().get(header::COOKIE)?.to_str().ok()?;
    auth::cookie_value(cookies, auth::COOKIE).map(str::to_string)
}

const PUBLIC_API: &[&str] = &[
    "/api/session",
    "/api/login",
    "/api/cybergrid/themes",
    "/api/cybergrid/events",
];

async fn guard(State(s): State<Shared>, req: Request, next: Next) -> Response {
    let hosts = allowed_hosts(&s);
    let host = req
        .headers()
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    if !hosts.iter().any(|h| h == host) {
        return (StatusCode::MISDIRECTED_REQUEST, "unexpected Host header").into_response();
    }
    let local_trusted = local_trusted(&s, &req, host);
    let path = req.uri().path().to_string();
    let mutating = !matches!(*req.method(), Method::GET | Method::HEAD);
    if mutating
        && let Some(origin) = req
            .headers()
            .get(header::ORIGIN)
            .and_then(|o| o.to_str().ok())
    {
        let ok = hosts
            .iter()
            .any(|h| origin == format!("http://{h}") || origin == format!("https://{h}"));
        if !ok {
            return (StatusCode::FORBIDDEN, "cross-site request refused").into_response();
        }
    }
    let public = !path.starts_with("/api/")
        || PUBLIC_API.contains(&path.as_str())
        || path.starts_with("/api/cybergrid/css/");
    if !public && !local_trusted {
        let Some(session) = session_id(&req).and_then(|id| s.auth.get(&id)) else {
            return err(401, "sign in first");
        };
        if mutating {
            let sent = req
                .headers()
                .get("x-holodrive-csrf")
                .and_then(|t| t.to_str().ok())
                .unwrap_or("");
            if !auth::constant_time_eq(sent.as_bytes(), session.csrf.as_bytes()) {
                return err(403, "missing or stale CSRF token (reload the page)");
            }
        }
    }
    let raw = path == "/api/raw" || path == "/api/snap/raw";
    let mut resp = next.run(req).await;
    let hs = resp.headers_mut();
    hs.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    hs.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    hs.insert("x-frame-options", HeaderValue::from_static("SAMEORIGIN"));
    if !raw && !hs.contains_key("content-security-policy") {
        hs.insert(
            "content-security-policy",
            HeaderValue::from_static(
                "default-src 'self'; img-src 'self' data: blob:; media-src 'self' blob:; \
                 style-src 'self' 'unsafe-inline'; script-src 'self'; frame-src 'self'; \
                 connect-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'self'",
            ),
        );
    }
    resp
}

fn err(code: u16, msg: impl ToString) -> Response {
    let status = StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, Json(json!({ "error": msg.to_string() }))).into_response()
}

fn fs_err(e: FsError) -> Response {
    err(e.0, e.1)
}

/// Run blocking filesystem work off the async threads.
#[allow(clippy::result_large_err)]
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, FsError> + Send + 'static,
) -> Result<T, Response> {
    match tokio::task::spawn_blocking(f).await {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(e)) => Err(fs_err(e)),
        Err(e) => Err(err(500, e)),
    }
}

macro_rules! reply {
    ($e:expr) => {
        match $e.await {
            Ok(v) => Json(v).into_response(),
            Err(r) => r,
        }
    };
}

// ── pages and assets ───────────────────────────────────────────────────

async fn index() -> Html<&'static str> {
    Html(views::PAGE)
}

async fn asset(Path(file): Path<String>) -> Response {
    if file == "editor.js" {
        return (
            [
                (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
                (header::CONTENT_ENCODING, "gzip"),
                (header::CACHE_CONTROL, "public, max-age=604800, immutable"),
            ],
            views::EDITOR_JS_GZ,
        )
            .into_response();
    }
    let Some((body, ctype)) = views::asset(&file) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    (
        [
            (header::CONTENT_TYPE, ctype),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        body,
    )
        .into_response()
}

async fn font(Path(file): Path<String>) -> Response {
    let Some(body) = views::font(&file) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    (
        [
            (header::CONTENT_TYPE, "font/woff2"),
            (header::CACHE_CONTROL, "max-age=86400"),
        ],
        body,
    )
        .into_response()
}

async fn tokens_css() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, cybercore::tokens::CSS_CONTENT_TYPE)],
        cybercore::tokens::CSS,
    )
}

// ── session ────────────────────────────────────────────────────────────

fn host_name() -> String {
    std::fs::read_to_string("/etc/hostname")
        .unwrap_or_default()
        .trim()
        .to_string()
}

async fn session(State(s): State<Shared>, req: Request) -> Json<Value> {
    let host = req
        .headers()
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    let local_trusted = local_trusted(&s, &req, host);
    let sess = session_id(&req).and_then(|id| s.auth.get(&id));
    Json(json!({
        "authed": local_trusted || sess.is_some(),
        "local": local_trusted,
        "csrf": sess.map(|x| x.csrf),
        "configured": !s.cfg.password_hash.is_empty(),
        "host": host_name(),
        "user": std::env::var("USER").unwrap_or_default(),
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

#[derive(Deserialize)]
struct LoginBody {
    password: String,
}

async fn login(State(s): State<Shared>, Json(body): Json<LoginBody>) -> Response {
    if s.cfg.password_hash.is_empty() {
        return err(
            503,
            "no password set yet: run `holodrive passwd` on the host",
        );
    }
    if let LoginBrake::Wait(d) = s.auth.brake() {
        return err(
            429,
            format!(
                "too many wrong passwords; try again in {} s",
                d.as_secs() + 1
            ),
        );
    }
    let hash = s.cfg.password_hash.clone();
    let ok = tokio::task::spawn_blocking(move || auth::verify_password(&hash, &body.password))
        .await
        .unwrap_or(false);
    if !ok {
        s.auth.record_fail();
        return err(401, "wrong password");
    }
    let (id, sess) = s.auth.create();
    let cookie = format!(
        "{}={id}; HttpOnly; SameSite=Strict; Path=/; Max-Age={}",
        auth::COOKIE,
        s.auth.lifetime.as_secs()
    );
    (
        [(header::SET_COOKIE, cookie)],
        Json(json!({ "csrf": sess.csrf })),
    )
        .into_response()
}

async fn logout(State(s): State<Shared>, req: Request) -> Response {
    if let Some(id) = session_id(&req) {
        s.auth.remove(&id);
    }
    let cookie = format!(
        "{}=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0",
        auth::COOKIE
    );
    ([(header::SET_COOKIE, cookie)], Json(json!({ "ok": true }))).into_response()
}

// ── read API ───────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct PathQ {
    path: String,
}

#[derive(Deserialize)]
struct SearchQ {
    path: String,
    q: String,
}

async fn roots(State(s): State<Shared>) -> Response {
    let cfg = s.cfg.clone();
    reply!(blocking(move || Ok(fsops::roots(&cfg))))
}

async fn list(State(s): State<Shared>, Query(q): Query<PathQ>) -> Response {
    let cfg = s.cfg.clone();
    reply!(blocking(move || fsops::list(&cfg, &q.path)))
}

async fn stat(State(s): State<Shared>, Query(q): Query<PathQ>) -> Response {
    let cfg = s.cfg.clone();
    reply!(blocking(move || fsops::stat(&cfg, &q.path)))
}

/// Quiet existence probe for the command line (no 404s in the console).
async fn exists(State(s): State<Shared>, Query(q): Query<PathQ>) -> Response {
    let cfg = s.cfg.clone();
    reply!(blocking(move || {
        let r = fsops::resolve(&cfg, &q.path)?;
        let m = std::fs::metadata(&r.abs).ok();
        Ok(json!({ "exists": m.is_some(), "dir": m.as_ref().is_some_and(|m| m.is_dir()) }))
    }))
}

async fn search(State(s): State<Shared>, Query(q): Query<SearchQ>) -> Response {
    let cfg = s.cfg.clone();
    reply!(blocking(move || fsops::search(&cfg, &q.path, &q.q)))
}

async fn text_get(State(s): State<Shared>, Query(q): Query<PathQ>) -> Response {
    let cfg = s.cfg.clone();
    reply!(blocking(move || fsops::read_text(&cfg, &q.path)))
}

#[derive(Deserialize)]
struct TextBody {
    path: String,
    text: String,
    mtime: Option<u64>,
}

async fn text_put(State(s): State<Shared>, Json(b): Json<TextBody>) -> Response {
    let cfg = s.cfg.clone();
    reply!(blocking(move || {
        let m = fsops::write_text(&cfg, &b.path, &b.text, b.mtime)?;
        Ok(json!({ "mtime": m }))
    }))
}

#[derive(Deserialize)]
struct RawQ {
    path: String,
    #[serde(default)]
    dl: Option<String>,
}

/// File bytes with Range support (video seeking). Inline only for types a
/// browser shows without running page script; everything else downloads.
async fn raw(State(s): State<Shared>, Query(q): Query<RawQ>, req: Request) -> Response {
    let cfg = s.cfg.clone();
    let vpath = q.path.clone();
    let abs = match blocking(move || {
        let r = fsops::resolve(&cfg, &vpath)?;
        if !r.abs.is_file() {
            return Err(FsError(400, "not a file".into()));
        }
        Ok(r.abs)
    })
    .await
    {
        Ok(p) => p,
        Err(r) => return r,
    };
    serve_file(abs, q.dl.is_some(), req).await
}

async fn serve_file(abs: std::path::PathBuf, dl: bool, req: Request) -> Response {
    let name = abs
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mime = mime_guess::from_path(&abs).first_or_octet_stream();
    let essence = mime.essence_str().to_string();
    let inline_ok = !dl
        && (essence.starts_with("image/") && essence != "image/svg+xml"
            || essence.starts_with("video/")
            || essence.starts_with("audio/")
            || essence == "application/pdf"
            || essence.starts_with("text/") && essence != "text/html");
    let mut resp = match tower_http::services::ServeFile::new(&abs)
        .oneshot(req)
        .await
    {
        Ok(r) => r.map(Body::new),
        Err(e) => return err(500, e),
    };
    let hs = resp.headers_mut();
    let safe_name: String = name
        .chars()
        .filter(|c| *c != '"' && *c != '\\' && !c.is_control())
        .collect();
    let disposition = if inline_ok { "inline" } else { "attachment" };
    if let Ok(v) = HeaderValue::from_str(&format!(
        "{disposition}; filename=\"{}\"; filename*=UTF-8''{}",
        safe_name.escape_default(),
        pct(&name)
    )) {
        hs.insert(header::CONTENT_DISPOSITION, v);
    }
    if essence.starts_with("text/") && inline_ok {
        hs.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/plain; charset=utf-8"),
        );
    }
    if essence != "application/pdf" {
        hs.insert("content-security-policy", HeaderValue::from_static("sandbox; default-src 'none'; img-src 'self'; media-src 'self'; style-src 'unsafe-inline'"));
    }
    hs.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-cache"),
    );
    resp
}

fn pct(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

async fn thumb(State(s): State<Shared>, Query(q): Query<PathQ>) -> Response {
    let cfg = s.cfg.clone();
    let Ok(_permit) = s.thumbs.acquire().await else {
        return err(503, "busy");
    };
    let made = blocking(move || {
        let r = fsops::resolve(&cfg, &q.path)?;
        media::thumbnail(&r.abs).ok_or_else(|| FsError(404, "no thumbnail".into()))
    })
    .await;
    match made {
        Ok(p) => match tokio::fs::read(&p).await {
            Ok(bytes) => (
                [
                    (header::CONTENT_TYPE, "image/jpeg"),
                    (header::CACHE_CONTROL, "private, max-age=86400"),
                ],
                bytes,
            )
                .into_response(),
            Err(e) => err(500, e),
        },
        Err(r) => r,
    }
}

/// `GET /api/zip?p=<vpath>&p=<vpath>…` builds a zip and streams it.
async fn zip(State(s): State<Shared>, req: Request) -> Response {
    let paths: Vec<String> = req
        .uri()
        .query()
        .unwrap_or("")
        .split('&')
        .filter_map(|kv| kv.strip_prefix("p="))
        .map(unpct)
        .collect();
    if paths.is_empty() {
        return err(400, "nothing to zip");
    }
    let cfg = s.cfg.clone();
    let names = paths.clone();
    let built = blocking(move || {
        let abs: Vec<_> = names
            .iter()
            .map(|p| fsops::resolve(&cfg, p).map(|r| r.abs))
            .collect::<Result<_, _>>()?;
        media::zip_items(&abs).map_err(|e| FsError(500, e.to_string()))
    })
    .await;
    let file = match built {
        Ok(f) => f,
        Err(r) => return r,
    };
    let zip_name = if paths.len() == 1 {
        format!("{}.zip", paths[0].rsplit('/').next().unwrap_or("holodrive"))
    } else {
        "holodrive.zip".to_string()
    };
    let handle = match tokio::fs::File::open(&file).await {
        Ok(h) => h,
        Err(e) => return err(500, e),
    };
    let len = handle.metadata().await.map(|m| m.len()).unwrap_or(0);
    // Unlinking now is fine: the open handle keeps the data until streamed.
    let _ = tokio::fs::remove_file(&file).await;
    let stream = tokio_util_stream(handle);
    (
        [
            (header::CONTENT_TYPE, "application/zip".to_string()),
            (header::CONTENT_LENGTH, len.to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename*=UTF-8''{}", pct(&zip_name)),
            ),
        ],
        Body::from_stream(stream),
    )
        .into_response()
}

fn tokio_util_stream(
    file: tokio::fs::File,
) -> impl futures_util::Stream<Item = std::io::Result<Vec<u8>>> {
    futures_util::stream::unfold(file, |mut f| async move {
        use tokio::io::AsyncReadExt;
        let mut buf = vec![0u8; 256 * 1024];
        match f.read(&mut buf).await {
            Ok(0) => None,
            Ok(n) => {
                buf.truncate(n);
                Some((Ok(buf), f))
            }
            Err(e) => Some((Err(e), f)),
        }
    })
}

fn unpct(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                match u8::from_str_radix(
                    std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("zz"),
                    16,
                ) {
                    Ok(b) => {
                        out.push(b);
                        i += 2;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

// ── write API ──────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct DirName {
    dir: String,
    name: String,
}

async fn mkdir(State(s): State<Shared>, Json(b): Json<DirName>) -> Response {
    let cfg = s.cfg.clone();
    reply!(blocking(
        move || fsops::mkdir(&cfg, &b.dir, &b.name).map(|p| json!({ "path": p }))
    ))
}

async fn newfile(State(s): State<Shared>, Json(b): Json<DirName>) -> Response {
    let cfg = s.cfg.clone();
    reply!(blocking(move || {
        fsops::check_name(&b.name)?;
        let (path, vpath) = fsops::upload_target(&cfg, &b.dir, &b.name, false)?;
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?;
        Ok(json!({ "path": vpath }))
    }))
}

#[derive(Deserialize)]
struct RenameBody {
    path: String,
    name: String,
}

async fn rename(State(s): State<Shared>, Json(b): Json<RenameBody>) -> Response {
    let cfg = s.cfg.clone();
    reply!(blocking(
        move || fsops::rename(&cfg, &b.path, &b.name).map(|p| json!({ "path": p }))
    ))
}

#[derive(Deserialize)]
struct TransferBody {
    paths: Vec<String>,
    dest: String,
    #[serde(default)]
    copy: bool,
}

async fn transfer(State(s): State<Shared>, Json(b): Json<TransferBody>) -> Response {
    let cfg = s.cfg.clone();
    reply!(blocking(move || fsops::transfer(
        &cfg, &b.paths, &b.dest, b.copy
    )
    .map(|p| json!({ "paths": p }))))
}

#[derive(Deserialize)]
struct PathsBody {
    paths: Vec<String>,
}

async fn trash(State(s): State<Shared>, Json(b): Json<PathsBody>) -> Response {
    let cfg = s.cfg.clone();
    reply!(blocking(
        move || fsops::trash(&cfg, &b.paths).map(|t| json!({ "trashed": t }))
    ))
}

async fn trash_list(State(s): State<Shared>) -> Response {
    let cfg = s.cfg.clone();
    reply!(blocking(move || Ok(
        json!({ "items": fsops::trash_list(&cfg), "days": cfg.trash_days })
    )))
}

#[derive(Deserialize)]
struct TrashRef {
    root: String,
    id: String,
}

async fn restore(State(s): State<Shared>, Json(b): Json<TrashRef>) -> Response {
    let cfg = s.cfg.clone();
    reply!(blocking(
        move || fsops::restore(&cfg, &b.root, &b.id).map(|p| json!({ "path": p }))
    ))
}

async fn purge(State(s): State<Shared>, Json(b): Json<TrashRef>) -> Response {
    let cfg = s.cfg.clone();
    reply!(blocking(
        move || fsops::purge(&cfg, &b.root, &b.id).map(|_| json!({ "ok": true }))
    ))
}

#[derive(Deserialize)]
struct UploadQ {
    dir: String,
    name: String,
    #[serde(default)]
    overwrite: Option<String>,
}

/// The body is the raw file. It streams to a hidden temp file next to the
/// target and is renamed into place only when complete.
async fn upload(State(s): State<Shared>, Query(q): Query<UploadQ>, body: Body) -> Response {
    let cfg = s.cfg.clone();
    let overwrite = q.overwrite.is_some();
    let target = blocking(move || fsops::upload_target(&cfg, &q.dir, &q.name, overwrite)).await;
    let (path, vpath) = match target {
        Ok(t) => t,
        Err(r) => return r,
    };
    let tmp = path.with_file_name(format!(".holodrive-upload-{}", auth::random_hex(6)));
    let result: std::io::Result<u64> = async {
        let mut f = tokio::fs::File::create(&tmp).await?;
        let mut n = 0u64;
        let mut stream = body.into_data_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(std::io::Error::other)?;
            n += chunk.len() as u64;
            f.write_all(&chunk).await?;
        }
        f.flush().await?;
        f.sync_all().await?;
        Ok(n)
    }
    .await;
    match result {
        Ok(n) => match tokio::fs::rename(&tmp, &path).await {
            Ok(()) => Json(json!({ "path": vpath, "size": n })).into_response(),
            Err(e) => {
                let _ = tokio::fs::remove_file(&tmp).await;
                err(500, e)
            }
        },
        Err(e) => {
            let _ = tokio::fs::remove_file(&tmp).await;
            err(500, format!("upload interrupted: {e}"))
        }
    }
}

#[derive(Deserialize)]
struct StartBody {
    dir: String,
    name: String,
    size: u64,
    #[serde(default)]
    overwrite: bool,
}

async fn upload_start(State(s): State<Shared>, Json(b): Json<StartBody>) -> Response {
    let cfg = s.cfg.clone();
    reply!(blocking(move || uploads::start(
        &cfg,
        &b.dir,
        &b.name,
        b.size,
        b.overwrite
    )))
}

#[derive(Deserialize)]
struct IdQ {
    id: String,
}

async fn upload_status(State(s): State<Shared>, Query(q): Query<IdQ>) -> Response {
    let cfg = s.cfg.clone();
    reply!(blocking(move || uploads::status(&cfg, &q.id)))
}

async fn upload_finish(State(s): State<Shared>, Json(b): Json<IdQ>) -> Response {
    let cfg = s.cfg.clone();
    reply!(blocking(move || uploads::finish(&cfg, &b.id)))
}

async fn upload_cancel(State(s): State<Shared>, Json(b): Json<IdQ>) -> Response {
    let cfg = s.cfg.clone();
    reply!(blocking(move || uploads::cancel(&cfg, &b.id)))
}

/// Marks an upload as receiving a chunk; one writer per upload at a time.
struct Sending(Shared, String);

impl Sending {
    fn claim(s: &Shared, id: &str) -> Option<Sending> {
        let mut set = s.sending.lock().unwrap_or_else(|e| e.into_inner());
        set.insert(id.to_string())
            .then(|| Sending(s.clone(), id.to_string()))
    }
}

impl Drop for Sending {
    fn drop(&mut self) {
        let mut set = self.0.sending.lock().unwrap_or_else(|e| e.into_inner());
        set.remove(&self.1);
    }
}

#[derive(Deserialize)]
struct ChunkQ {
    id: String,
    offset: u64,
}

fn at_offset(code: StatusCode, msg: impl ToString, offset: u64) -> Response {
    (
        code,
        Json(json!({ "error": msg.to_string(), "offset": offset })),
    )
        .into_response()
}

/// The body is the next piece of the file, starting at `offset`. Bytes that
/// arrive before a connection drops are kept; the client asks `status` (or
/// reads `offset` from a 409) and carries on from there.
async fn upload_chunk(State(s): State<Shared>, Query(q): Query<ChunkQ>, body: Body) -> Response {
    let cfg = s.cfg.clone();
    let id = q.id.clone();
    let up = match blocking(move || uploads::find(&cfg, &id)).await {
        Ok(u) => u,
        Err(r) => return r,
    };
    let Some(_sending) = Sending::claim(&s, &q.id) else {
        return err(409, "this upload is already receiving a chunk");
    };
    let have = match tokio::fs::metadata(&up.part).await {
        Ok(m) => m.len(),
        Err(e) => return err(500, e),
    };
    if have != q.offset {
        return at_offset(
            StatusCode::CONFLICT,
            format!("the server has {have} bytes; continue from there"),
            have,
        );
    }
    let room = up.size() - have;
    let mut n = 0u64;
    let result: std::io::Result<()> = async {
        let mut f = tokio::fs::OpenOptions::new()
            .append(true)
            .open(&up.part)
            .await?;
        let mut stream = body.into_data_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(std::io::Error::other)?;
            if n + chunk.len() as u64 > room {
                f.sync_data().await?;
                return Err(std::io::Error::new(
                    std::io::ErrorKind::FileTooLarge,
                    "more data than the size given at start",
                ));
            }
            f.write_all(&chunk).await?;
            n += chunk.len() as u64;
        }
        f.sync_data().await
    }
    .await;
    match result {
        Ok(()) => Json(json!({ "offset": have + n, "size": up.size() })).into_response(),
        Err(e) if e.kind() == std::io::ErrorKind::FileTooLarge => {
            at_offset(StatusCode::PAYLOAD_TOO_LARGE, e, have + n)
        }
        Err(e) => at_offset(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("chunk interrupted: {e}"),
            have + n,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;

    #[test]
    fn unpct_decodes() {
        assert_eq!(super::unpct("a%2Fb%20c+d"), "a/b c d");
        assert_eq!(super::unpct("100%"), "100%");
        assert_eq!(super::unpct("%zz"), "%zz");
        assert_eq!(super::pct("a b/ü"), "a%20b%2F%C3%BC");
    }

    #[tokio::test]
    async fn chunked_upload_resumes_and_refuses_bad_offsets() {
        let dir = std::env::temp_dir().join(format!("holodrive-web-up-{}", auth::random_hex(4)));
        std::fs::create_dir_all(dir.join("a")).unwrap();
        let mut cfg = Config::starter();
        cfg.password_hash = "configured".into();
        cfg.local_auth_bypass = true;
        cfg.roots = vec![crate::config::Root {
            id: "a".into(),
            name: "A".into(),
            path: dir.join("a"),
            icon: String::new(),
            writable: true,
        }];
        let app = router(Arc::new(AppState {
            cfg: Arc::new(cfg),
            auth: Auth::new(30),
            port: 8768,
            thumbs: tokio::sync::Semaphore::new(2),
            stopping: tokio::sync::watch::channel(false).1,
            sending: Default::default(),
        }));
        let call = |method: &str, uri: String, body: Body, json: bool| {
            let mut b = Request::builder()
                .method(method)
                .uri(uri)
                .header(header::HOST, "127.0.0.1:8768");
            if json {
                b = b.header(header::CONTENT_TYPE, "application/json");
            }
            app.clone().oneshot(b.body(body).unwrap())
        };
        let read = |r: Response| async move {
            let status = r.status();
            let bytes = axum::body::to_bytes(r.into_body(), usize::MAX)
                .await
                .unwrap();
            (status, serde_json::from_slice::<Value>(&bytes).unwrap())
        };

        let start = json!({ "dir": "a", "name": "movie.mkv", "size": 10 }).to_string();
        let (st, v) = read(
            call("POST", "/api/upload/start".into(), Body::from(start), true)
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        let id = v["id"].as_str().unwrap().to_string();
        let chunk = |offset: u64, data: &'static [u8]| {
            call(
                "PUT",
                format!("/api/upload/chunk?id={id}&offset={offset}"),
                Body::from(data),
                false,
            )
        };

        let (st, v) = read(chunk(0, b"01234").await.unwrap()).await;
        assert_eq!((st, v["offset"].as_u64()), (StatusCode::OK, Some(5)));
        // A client that lost track (e.g. resent the first chunk) is told where to go on.
        let (st, v) = read(chunk(0, b"01234").await.unwrap()).await;
        assert_eq!((st, v["offset"].as_u64()), (StatusCode::CONFLICT, Some(5)));
        // More than was announced is refused; what fit is kept.
        let (st, v) = read(chunk(5, b"56789XX").await.unwrap()).await;
        assert_eq!(st, StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(v["offset"].as_u64(), Some(5));
        let (st, v) = read(
            call(
                "GET",
                format!("/api/upload/status?id={id}"),
                Body::empty(),
                false,
            )
            .await
            .unwrap(),
        )
        .await;
        assert_eq!((st, v["offset"].as_u64()), (StatusCode::OK, Some(5)));
        let (st, _) = read(chunk(5, b"56789").await.unwrap()).await;
        assert_eq!(st, StatusCode::OK);
        let fin = json!({ "id": id }).to_string();
        let (st, v) = read(
            call("POST", "/api/upload/finish".into(), Body::from(fin), true)
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(
            (st, v["path"].as_str()),
            (StatusCode::OK, Some("a/movie.mkv"))
        );
        assert_eq!(
            std::fs::read(dir.join("a/movie.mkv")).unwrap(),
            b"0123456789"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn theme_events_end_when_the_server_stops() {
        let mut cfg = Config::starter();
        cfg.password_hash = "configured".into();
        cfg.local_auth_bypass = true;
        let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
        let state = Arc::new(AppState {
            cfg: Arc::new(cfg),
            auth: Auth::new(30),
            port: 8768,
            thumbs: tokio::sync::Semaphore::new(2),
            stopping: stop_rx,
            sending: Default::default(),
        });
        let res = router(state)
            .oneshot(
                Request::builder()
                    .uri("/api/cybergrid/events")
                    .header(header::HOST, "127.0.0.1:8768")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let mut body = res.into_body().into_data_stream();
        let drain = async {
            while let Some(chunk) = body.next().await {
                chunk.unwrap();
            }
        };
        tokio::pin!(drain);
        let open = tokio::time::timeout(std::time::Duration::from_millis(300), &mut drain).await;
        assert!(open.is_err(), "stream ended before shutdown");
        stop_tx.send(true).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), drain)
            .await
            .expect("stream still open after shutdown");
    }

    #[tokio::test]
    async fn trusted_local_access_skips_login_but_tailnet_still_requires_it() {
        let mut cfg = Config::starter();
        cfg.password_hash = "configured".into();
        cfg.local_auth_bypass = true;
        cfg.hosts.push("blackbox.tail9311b5.ts.net:8768".into());
        let state = Arc::new(AppState {
            cfg: Arc::new(cfg),
            auth: Auth::new(30),
            port: 8768,
            thumbs: tokio::sync::Semaphore::new(2),
            stopping: tokio::sync::watch::channel(false).1,
            sending: Default::default(),
        });
        let app = router(state);

        let local = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/roots")
                    .header(header::HOST, "127.0.0.1:8768")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(local.status(), StatusCode::OK);

        let proxied = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/roots")
                    .header(header::HOST, "127.0.0.1:8768")
                    .header("x-forwarded-for", "100.64.0.7")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(proxied.status(), StatusCode::UNAUTHORIZED);

        let tailnet = app
            .oneshot(
                Request::builder()
                    .uri("/api/roots")
                    .header(header::HOST, "blackbox.tail9311b5.ts.net:8768")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(tailnet.status(), StatusCode::UNAUTHORIZED);
    }
}

// ── Samba panel ────────────────────────────────────────────────────────

async fn samba_overview(State(s): State<Shared>) -> Response {
    let cfg = s.cfg.clone();
    reply!(blocking(move || Ok(samba::overview(&cfg))))
}

fn helper_reply(r: Result<(String, Value), String>) -> Result<Value, FsError> {
    r.map(|(message, data)| json!({ "message": message, "data": data }))
        .map_err(|e| FsError(422, e))
}

#[derive(Deserialize)]
struct ShareBody {
    name: String,
    #[serde(default)]
    original: Option<String>,
    /// A Holodrive path (`shared/x`) or an absolute path.
    path: String,
    #[serde(default)]
    comment: String,
    #[serde(default)]
    read_only: bool,
    #[serde(default = "yes")]
    browseable: bool,
    #[serde(default = "yes")]
    available: bool,
}
fn yes() -> bool {
    true
}

async fn samba_share(State(s): State<Shared>, Json(b): Json<ShareBody>) -> Response {
    let cfg = s.cfg.clone();
    reply!(blocking(move || {
        let path = if b.path.starts_with('/') {
            b.path.clone()
        } else {
            fsops::resolve(&cfg, &b.path)?
                .abs
                .to_string_lossy()
                .into_owned()
        };
        let edit = smbconf::Edit {
            name: b.name.trim().to_string(),
            path: path.clone(),
            comment: b.comment.trim().to_string(),
            read_only: b.read_only,
            browseable: b.browseable,
            available: b.available,
        };
        // Dry run against the current file for a quick, friendly error.
        let current = std::fs::read_to_string("/etc/samba/smb.conf").unwrap_or_default();
        smbconf::upsert(&current, &edit, b.original.as_deref()).map_err(|e| FsError(400, e))?;
        let body = json!({
            "name": edit.name, "original": b.original, "path": path, "comment": edit.comment,
            "read_only": edit.read_only, "browseable": edit.browseable, "available": edit.available,
        });
        helper_reply(samba::helper(&["upsert"], Some(body.to_string())))
    }))
}

#[derive(Deserialize)]
struct NameBody {
    name: String,
}

async fn samba_remove(State(_s): State<Shared>, Json(b): Json<NameBody>) -> Response {
    reply!(blocking(move || {
        let current = std::fs::read_to_string("/etc/samba/smb.conf").unwrap_or_default();
        smbconf::remove(&current, &b.name).map_err(|e| FsError(400, e))?;
        helper_reply(samba::helper(&["remove", &b.name], None))
    }))
}

#[derive(Deserialize)]
struct ActionBody {
    action: String,
    #[serde(default)]
    name: String,
}

async fn samba_action(State(_s): State<Shared>, Json(b): Json<ActionBody>) -> Response {
    reply!(blocking(move || match b.action.as_str() {
        "reload" | "restart" => helper_reply(samba::helper(&[&b.action], None)),
        "close-share" => {
            smbconf::valid_share_name(&b.name).map_err(|e| FsError(400, e))?;
            helper_reply(samba::helper(&["close-share", &b.name], None))
        }
        _ => Err(FsError(400, "unknown action".into())),
    }))
}

async fn samba_backups(State(_s): State<Shared>) -> Response {
    reply!(blocking(move || helper_reply(samba::helper(
        &["backups"],
        None
    ))))
}

async fn samba_restore(State(_s): State<Shared>, Json(b): Json<NameBody>) -> Response {
    reply!(blocking(move || helper_reply(samba::helper(
        &["restore", &b.name],
        None
    ))))
}

// ── time machine ───────────────────────────────────────────────────────

#[derive(Deserialize)]
struct SnapQ {
    path: String,
    snap: String,
    #[serde(default)]
    dl: Option<String>,
}

async fn snap_timeline(State(s): State<Shared>, Query(q): Query<PathQ>) -> Response {
    let cfg = s.cfg.clone();
    reply!(blocking(move || snapshots::timeline(&cfg, &q.path)))
}

async fn snap_list(State(s): State<Shared>, Query(q): Query<SnapQ>) -> Response {
    let cfg = s.cfg.clone();
    reply!(blocking(move || snapshots::list(&cfg, &q.path, &q.snap)))
}

async fn snap_versions(State(s): State<Shared>, Query(q): Query<PathQ>) -> Response {
    let cfg = s.cfg.clone();
    reply!(blocking(move || snapshots::versions(&cfg, &q.path)))
}

async fn snap_text(State(s): State<Shared>, Query(q): Query<SnapQ>) -> Response {
    let cfg = s.cfg.clone();
    reply!(blocking(move || snapshots::read_text(
        &cfg, &q.path, &q.snap
    )))
}

async fn snap_raw(State(s): State<Shared>, Query(q): Query<SnapQ>, req: Request) -> Response {
    let cfg = s.cfg.clone();
    let dl = q.dl.is_some();
    let found = blocking(move || {
        let p = snapshots::file(&cfg, &q.path, &q.snap)?;
        if !p.is_file() {
            return Err(FsError(400, "not a file".into()));
        }
        Ok(p)
    })
    .await;
    match found {
        Ok(p) => serve_file(p, dl, req).await,
        Err(r) => r,
    }
}

async fn snap_thumb(State(s): State<Shared>, Query(q): Query<SnapQ>) -> Response {
    let cfg = s.cfg.clone();
    let Ok(_permit) = s.thumbs.acquire().await else {
        return err(503, "busy");
    };
    let made = blocking(move || {
        let p = snapshots::file(&cfg, &q.path, &q.snap)?;
        media::thumbnail(&p).ok_or_else(|| FsError(404, "no thumbnail".into()))
    })
    .await;
    match made {
        Ok(p) => match tokio::fs::read(&p).await {
            Ok(bytes) => (
                [
                    (header::CONTENT_TYPE, "image/jpeg"),
                    (header::CACHE_CONTROL, "private, max-age=86400"),
                ],
                bytes,
            )
                .into_response(),
            Err(e) => err(500, e),
        },
        Err(r) => r,
    }
}

#[derive(Deserialize)]
struct RestoreBody {
    path: String,
    snap: String,
    #[serde(default)]
    replace: bool,
    #[serde(default)]
    label: String,
}

async fn snap_restore(State(s): State<Shared>, Json(b): Json<RestoreBody>) -> Response {
    let cfg = s.cfg.clone();
    reply!(blocking(move || snapshots::restore(
        &cfg, &b.path, &b.snap, b.replace, &b.label
    )))
}

async fn samba_conf(State(_s): State<Shared>) -> Response {
    reply!(blocking(move || {
        let text = std::fs::read_to_string("/etc/samba/smb.conf")
            .map_err(|e| FsError(503, format!("/etc/samba/smb.conf: {e}")))?;
        let rails = smbconf::check_rails(&text);
        Ok(json!({
            "text": text,
            "hash": smbconf::content_hash(&text),
            "helper": samba::helper_installed(),
            "warnings": rails.as_ref().ok(),
            "errors": rails.as_ref().err(),
        }))
    }))
}

#[derive(Deserialize)]
struct ConfBody {
    text: String,
    hash: String,
    #[serde(default)]
    accept_warnings: bool,
}

/// Save the whole file. Errors from the rails come back as a list (400);
/// warnings need `accept_warnings` (428) so the page can ask first.
async fn samba_conf_write(State(_s): State<Shared>, Json(b): Json<ConfBody>) -> Response {
    let checked = tokio::task::spawn_blocking(move || {
        let home = dirs::home_dir().unwrap_or_default();
        let outside: Vec<String> = smbconf::share_paths(&b.text)
            .into_iter()
            .filter(|(_, p)| {
                !std::path::Path::new(p).starts_with(&home) || !std::path::Path::new(p).is_dir()
            })
            .map(|(sec, p)| {
                format!(
                    "[{sec}] path {p}: must be an existing folder inside {}",
                    home.display()
                )
            })
            .collect();
        if !outside.is_empty() {
            return Err((
                400,
                json!({ "error": "refused by Holodrive's safety rails", "errors": outside }),
            ));
        }
        match smbconf::check_rails(&b.text) {
            Err(errors) => Err((
                400,
                json!({ "error": "refused by Holodrive's safety rails", "errors": errors }),
            )),
            Ok(w) if !w.is_empty() && !b.accept_warnings => {
                Err((428, json!({ "error": "please confirm", "warnings": w })))
            }
            Ok(_) => match samba::helper(&["write", &b.hash], Some(b.text)) {
                Ok((message, data)) => Ok(json!({ "message": message, "data": data })),
                Err(e) => Err((422, json!({ "error": e }))),
            },
        }
    })
    .await;
    match checked {
        Ok(Ok(v)) => Json(v).into_response(),
        Ok(Err((code, v))) => (
            StatusCode::from_u16(code).unwrap_or(StatusCode::BAD_REQUEST),
            Json(v),
        )
            .into_response(),
        Err(e) => err(500, e),
    }
}
