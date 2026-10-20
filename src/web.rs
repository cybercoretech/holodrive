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
use crate::{
    cybergrid, cybervault, ghostport, links, media, samba, smbconf, snapshots, tokens, uploads,
    views,
};
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
    /// The theme catalog's revision (see `cybergrid::watch_revision`).
    pub themes: tokio::sync::watch::Receiver<Option<u64>>,
    /// Resumable uploads with a chunk being written right now.
    pub sending: std::sync::Mutex<std::collections::HashSet<String>>,
    pub tokens: tokens::Store,
    pub links: links::Store,
}

type Shared = Arc<AppState>;

async fn ghostport_status() -> Json<ghostport::IntegrationStatus> {
    Json(ghostport::status().await)
}

async fn cybervault_status() -> Json<cybervault::Status> {
    Json(cybervault::status().await)
}

async fn cybervault_store(Json(body): Json<cybervault::StoreRequest>) -> Response {
    match cybervault::store(body).await {
        Ok(reply) => Json(reply).into_response(),
        Err(reason) => err(503, reason),
    }
}

pub fn router(state: Shared) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/static/:file", get(asset))
        .route("/static/fonts/:file", get(font))
        .route("/vendor/tokens.css", get(tokens_css))
        .route("/api/cybergrid/themes", get(cybergrid::list_themes))
        .route("/api/cybergrid/events", get(cybergrid::theme_events))
        .route("/api/cybergrid/css/:name", get(cybergrid::theme_css))
        .route("/api/integrations/ghostport/status", get(ghostport_status))
        .route(
            "/api/integrations/cybervault/status",
            get(cybervault_status),
        )
        .route("/api/integrations/cybervault/store", post(cybervault_store))
        .route("/api/session", get(session))
        .route("/api/login", post(login))
        .route("/api/logout", post(logout))
        .route("/api/roots", get(roots))
        .route("/api/list", get(list))
        .route("/api/photos", get(photos))
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
        .route("/api/tokens", get(tokens_list).post(tokens_create))
        .route("/api/tokens/revoke", post(tokens_revoke))
        .route("/api/links", get(links_list).post(links_create))
        .route("/api/links/revoke", post(links_revoke))
        .route("/s/:key", get(link_page))
        .route("/api/s/:key", get(link_info))
        .route("/api/s/:key/unlock", post(link_unlock))
        .route("/api/s/:key/list", get(link_list))
        .route("/api/s/:key/raw", get(link_raw))
        .route("/api/s/:key/zip", get(link_zip))
        .route(
            "/api/s/:key/upload",
            post(link_upload).layer(DefaultBodyLimit::disable()),
        )
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

fn bearer(req: &Request) -> Option<&str> {
    req.headers()
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::trim)
}

/// Where a request came from, for a token's "last used" line.
fn origin_label(req: &Request) -> String {
    let h = |name: &str| {
        req.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    };
    h("tailscale-user-login")
        .or_else(|| h("x-forwarded-for"))
        .unwrap_or_else(|| "this PC".into())
}

/// Only a browser session (or this PC) may manage Samba and device tokens.
const SESSION_ONLY: &[&str] = &[
    "/api/samba",
    "/api/tokens",
    "/api/links",
    "/api/integrations/cybervault",
];

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
        || path.starts_with("/api/cybergrid/css/")
        // Share links check their own key, expiry and password.
        || path.starts_with("/api/s/");
    if !public && let Some(secret) = bearer(&req) {
        let Some(grant) = s.tokens.verify(secret, &origin_label(&req)) else {
            return err(401, "unknown or revoked device token");
        };
        if SESSION_ONLY.iter().any(|p| path.starts_with(p)) {
            return err(403, "device tokens can't manage Samba, tokens or links");
        }
        if mutating && grant.read_only {
            return err(403, format!("the token for {} is read-only", grant.name));
        }
    } else if !public && !local_trusted {
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
    let device = bearer(&req).and_then(|t| s.tokens.verify(t, &origin_label(&req)));
    Json(json!({
        "authed": local_trusted || sess.is_some() || device.is_some(),
        "device": device.as_ref().map(|g| json!({ "name": g.name, "read_only": g.read_only })),
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

async fn photos(State(s): State<Shared>, Query(q): Query<PathQ>) -> Response {
    let cfg = s.cfg.clone();
    reply!(blocking(move || fsops::photos(&cfg, &q.path)))
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
    zip_reply(s.cfg.clone(), paths).await
}

async fn zip_reply(cfg: Arc<Config>, paths: Vec<String>) -> Response {
    if paths.is_empty() {
        return err(400, "nothing to zip");
    }
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

/// The body is the raw file. It streams to a spool file on the same drive
/// (see `uploads`) and is moved into place only when complete.
async fn upload(State(s): State<Shared>, Query(q): Query<UploadQ>, body: Body) -> Response {
    store_upload(s.cfg.clone(), q.dir, q.name, q.overwrite.is_some(), body).await
}

async fn store_upload(
    cfg: Arc<Config>,
    dir: String,
    name: String,
    overwrite: bool,
    body: Body,
) -> Response {
    let target = blocking(move || {
        let (path, vpath) = fsops::upload_target(&cfg, &dir, &name, overwrite)?;
        let spool = uploads::spool_path(&cfg, &vpath)?;
        Ok((path, vpath, spool))
    })
    .await;
    let (path, vpath, tmp) = match target {
        Ok(t) => t,
        Err(r) => return r,
    };
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
    let n = match result {
        Ok(n) => n,
        Err(e) => {
            let _ = tokio::fs::remove_file(&tmp).await;
            return err(500, format!("upload interrupted: {e}"));
        }
    };
    let spool = tmp.clone();
    match blocking(move || uploads::move_into_place(&spool, &path)).await {
        Ok(()) => Json(json!({ "path": vpath, "size": n })).into_response(),
        Err(r) => {
            let _ = tokio::fs::remove_file(&tmp).await;
            r
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

async fn tokens_list(State(s): State<Shared>) -> Json<Value> {
    let list: Vec<Value> = s.tokens.list().iter().map(|t| t.public()).collect();
    Json(json!({ "tokens": list }))
}

#[derive(Deserialize)]
struct NewToken {
    name: String,
    #[serde(default)]
    read_only: bool,
}

async fn tokens_create(State(s): State<Shared>, Json(b): Json<NewToken>) -> Response {
    match s.tokens.create(&b.name, b.read_only) {
        Ok((t, secret)) => {
            let mut v = t.public();
            v["token"] = json!(secret);
            Json(v).into_response()
        }
        Err(e) => err(400, e),
    }
}

async fn tokens_revoke(State(s): State<Shared>, Json(b): Json<IdQ>) -> Response {
    match s.tokens.revoke(&b.id) {
        Ok(true) => Json(json!({ "ok": true })).into_response(),
        Ok(false) => err(404, "no such token"),
        Err(e) => err(500, e),
    }
}

// ── share links ─────────────────────────────────────────────────────────

/// The address to hand out: the first tailnet name, else wherever the
/// owner is looking from.
fn link_base(s: &AppState) -> Option<String> {
    s.cfg.hosts.first().map(|h| format!("http://{h}"))
}

async fn links_list(State(s): State<Shared>) -> Json<Value> {
    let list: Vec<Value> = s.links.list().iter().map(|l| l.public()).collect();
    Json(json!({ "links": list, "base": link_base(&s) }))
}

async fn links_create(State(s): State<Shared>, Json(b): Json<links::NewLink>) -> Response {
    let st = s.clone();
    match blocking(move || st.links.create(&st.cfg, b)).await {
        Ok(l) => {
            let mut v = l.public();
            v["base"] = json!(link_base(&s));
            Json(v).into_response()
        }
        Err(r) => r,
    }
}

#[derive(Deserialize)]
struct KeyBody {
    key: String,
}

async fn links_revoke(State(s): State<Shared>, Json(b): Json<KeyBody>) -> Response {
    match s.links.revoke(&b.key) {
        Ok(true) => Json(json!({ "ok": true })).into_response(),
        Ok(false) => err(404, "no such link"),
        Err(e) => err(500, e),
    }
}

async fn link_page() -> Html<&'static str> {
    Html(views::LINK_PAGE)
}

const LINK_COOKIE: &str = "holodrive_link";

/// The link behind a public request, if it's live and (when it has a
/// password) unlocked in this browser.
#[allow(clippy::result_large_err)]
fn open_link(
    s: &AppState,
    key: &str,
    req_headers: &axum::http::HeaderMap,
) -> Result<links::Link, Response> {
    let Some(link) = s.links.get(key) else {
        return Err(err(404, "this link doesn't exist or was revoked"));
    };
    if link.expired() {
        return Err(err(410, "this link has expired"));
    }
    if link.has_password() {
        let grant = req_headers
            .get(header::COOKIE)
            .and_then(|c| c.to_str().ok())
            .and_then(|c| auth::cookie_value(c, LINK_COOKIE));
        if !grant.is_some_and(|g| s.links.granted(key, g)) {
            return Err((
                StatusCode::UNAUTHORIZED,
                Json(json!({ "error": "this link needs its password", "locked": true })),
            )
                .into_response());
        }
    }
    Ok(link)
}

async fn link_info(State(s): State<Shared>, Path(key): Path<String>, req: Request) -> Response {
    let link = match open_link(&s, &key, req.headers()) {
        Ok(l) => l,
        Err(r) if r.status() == StatusCode::UNAUTHORIZED => {
            // Say only that it exists and needs a password.
            return Json(json!({ "locked": true })).into_response();
        }
        Err(r) => return r,
    };
    let size = if link.dir {
        None
    } else {
        let cfg = s.cfg.clone();
        let path = link.path.clone();
        blocking(move || Ok(std::fs::metadata(fsops::resolve(&cfg, &path)?.abs)?.len()))
            .await
            .ok()
    };
    Json(json!({
        "locked": false,
        "name": link.name(),
        "dir": link.dir,
        "mode": link.mode,
        "label": link.label,
        "expires": link.expires,
        "size": size,
        "downloads_left": link.max_downloads.map(|m| m.saturating_sub(link.downloads)),
    }))
    .into_response()
}

#[derive(Deserialize)]
struct PasswordBody {
    password: String,
}

async fn link_unlock(
    State(s): State<Shared>,
    Path(key): Path<String>,
    Json(b): Json<PasswordBody>,
) -> Response {
    let st = s.clone();
    let k = key.clone();
    let result = tokio::task::spawn_blocking(move || st.links.unlock(&k, &b.password)).await;
    match result {
        Ok(links::Unlock::Granted(grant)) => {
            let cookie = format!(
                "{LINK_COOKIE}={grant}; HttpOnly; SameSite=Strict; Path=/api/s/{key}; Max-Age={}",
                links::GRANT_SECS
            );
            ([(header::SET_COOKIE, cookie)], Json(json!({ "ok": true }))).into_response()
        }
        Ok(links::Unlock::Wrong) => err(401, "wrong password"),
        Ok(links::Unlock::Wait(d)) => err(
            429,
            format!("too many tries; wait {} min", d.as_secs().div_ceil(60)),
        ),
        Err(e) => err(500, e),
    }
}

#[derive(Deserialize)]
struct SubQ {
    #[serde(default)]
    p: String,
    #[serde(default)]
    dl: Option<String>,
}

#[allow(clippy::result_large_err)]
fn viewable(
    s: &AppState,
    key: &str,
    headers: &axum::http::HeaderMap,
) -> Result<links::Link, Response> {
    let link = open_link(s, key, headers)?;
    if link.mode != links::Mode::View {
        return Err(err(403, "this is a drop box: you can upload, not look"));
    }
    Ok(link)
}

async fn link_list(
    State(s): State<Shared>,
    Path(key): Path<String>,
    Query(q): Query<SubQ>,
    req: Request,
) -> Response {
    let link = match viewable(&s, &key, req.headers()) {
        Ok(l) => l,
        Err(r) => return r,
    };
    let cfg = s.cfg.clone();
    reply!(blocking(move || {
        let v = fsops::list(&cfg, &link.target(&q.p)?)?;
        // Names and sizes only: no drive ids, real paths or modes.
        let entries: Vec<Value> = v["entries"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|e| e["escapes"] != json!(true))
            .map(|e| {
                json!({
                    "name": e["name"], "kind": e["kind"], "size": e["size"],
                    "mtime": e["mtime"], "ext": e["ext"], "mime": e["mime"],
                })
            })
            .collect();
        Ok(json!({ "entries": entries }))
    }))
}

/// A whole-file download (not a later range of a video being streamed).
fn starts_download(req: &Request, dl: bool) -> bool {
    dl || req
        .headers()
        .get(header::RANGE)
        .and_then(|r| r.to_str().ok())
        .is_none_or(|r| r.trim() == "bytes=0-")
}

async fn link_raw(
    State(s): State<Shared>,
    Path(key): Path<String>,
    Query(q): Query<SubQ>,
    req: Request,
) -> Response {
    let link = match viewable(&s, &key, req.headers()) {
        Ok(l) => l,
        Err(r) => return r,
    };
    let cfg = s.cfg.clone();
    let sub = q.p.clone();
    let abs = match blocking(move || {
        let r = fsops::resolve(&cfg, &link.target(&sub)?)?;
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
    let dl = q.dl.is_some();
    if starts_download(&req, dl) && !s.links.take_download(&key) {
        return err(410, "this link has no downloads left");
    }
    serve_file(abs, dl, req).await
}

async fn link_zip(
    State(s): State<Shared>,
    Path(key): Path<String>,
    Query(q): Query<SubQ>,
    req: Request,
) -> Response {
    let link = match viewable(&s, &key, req.headers()) {
        Ok(l) => l,
        Err(r) => return r,
    };
    let target = match link.target(&q.p) {
        Ok(t) => t,
        Err(e) => return fs_err(e),
    };
    if !s.links.take_download(&key) {
        return err(410, "this link has no downloads left");
    }
    zip_reply(s.cfg.clone(), vec![target]).await
}

#[derive(Deserialize)]
struct DropQ {
    name: String,
}

async fn link_upload(
    State(s): State<Shared>,
    Path(key): Path<String>,
    Query(q): Query<DropQ>,
    req: Request,
) -> Response {
    let link = match open_link(&s, &key, req.headers()) {
        Ok(l) => l,
        Err(r) => return r,
    };
    if link.mode != links::Mode::Drop {
        return err(403, "this link is for downloading");
    }
    if q.name.contains('/') {
        return err(400, "file names only, no folders");
    }
    let resp = store_upload(s.cfg.clone(), link.path, q.name, false, req.into_body()).await;
    if resp.status().is_success() {
        s.links.count_upload(&key);
        // The uploader learns only that it arrived, not where.
        return Json(json!({ "ok": true })).into_response();
    }
    resp
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
            themes: tokio::sync::watch::channel(None).1,
            sending: Default::default(),
            tokens: crate::tokens::Store::memory(),
            links: crate::links::Store::memory(),
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
    async fn device_tokens_authenticate_without_csrf_and_respect_limits() {
        let dir = std::env::temp_dir().join(format!("holodrive-web-tok-{}", auth::random_hex(4)));
        std::fs::create_dir_all(dir.join("a")).unwrap();
        let mut cfg = Config::starter();
        cfg.password_hash = "configured".into();
        cfg.hosts.push("blackbox.tail9311b5.ts.net:8768".into());
        cfg.roots = vec![crate::config::Root {
            id: "a".into(),
            name: "A".into(),
            path: dir.join("a"),
            icon: String::new(),
            writable: true,
        }];
        let store = crate::tokens::Store::memory();
        let (_, full) = store.create("Pixel", false).unwrap();
        let (ro_tok, ro) = store.create("Tablet", true).unwrap();
        let state = Arc::new(AppState {
            cfg: Arc::new(cfg),
            auth: Auth::new(30),
            port: 8768,
            thumbs: tokio::sync::Semaphore::new(2),
            stopping: tokio::sync::watch::channel(false).1,
            themes: tokio::sync::watch::channel(None).1,
            sending: Default::default(),
            tokens: store,
            links: crate::links::Store::memory(),
        });
        let app = router(state.clone());
        let call = |method: &str, uri: &str, token: Option<&str>, body: Option<Value>| {
            let mut b = Request::builder()
                .method(method)
                .uri(uri)
                .header(header::HOST, "blackbox.tail9311b5.ts.net:8768")
                .header("tailscale-user-login", "raven@github");
            if let Some(t) = token {
                b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
            }
            let body = match body {
                Some(v) => {
                    b = b.header(header::CONTENT_TYPE, "application/json");
                    Body::from(v.to_string())
                }
                None => Body::empty(),
            };
            app.clone().oneshot(b.body(body).unwrap())
        };
        let status = |r: Response| r.status();
        let mk = |name: &str| Some(json!({ "dir": "a", "name": name }));

        assert_eq!(
            status(call("GET", "/api/roots", None, None).await.unwrap()),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            status(call("GET", "/api/roots", Some(&full), None).await.unwrap()),
            StatusCode::OK
        );
        assert_eq!(
            status(
                call(
                    "POST",
                    "/api/integrations/cybervault/store",
                    Some(&full),
                    Some(json!({"label":"share", "secret":"sensitive"})),
                )
                .await
                .unwrap()
            ),
            StatusCode::FORBIDDEN
        );
        // No CSRF header needed with a bearer token.
        assert_eq!(
            status(
                call("POST", "/api/mkdir", Some(&full), mk("x"))
                    .await
                    .unwrap()
            ),
            StatusCode::OK
        );
        assert!(dir.join("a/x").is_dir());
        assert_eq!(
            status(
                call("GET", "/api/list?path=a", Some(&ro), None)
                    .await
                    .unwrap()
            ),
            StatusCode::OK
        );
        assert_eq!(
            status(
                call("POST", "/api/mkdir", Some(&ro), mk("y"))
                    .await
                    .unwrap()
            ),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            status(call("GET", "/api/tokens", Some(&full), None).await.unwrap()),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            status(call("GET", "/api/samba", Some(&full), None).await.unwrap()),
            StatusCode::FORBIDDEN
        );
        let forged = format!("hd_{}_{}", ro_tok.id, "f".repeat(64));
        assert_eq!(
            status(
                call("GET", "/api/roots", Some(&forged), None)
                    .await
                    .unwrap()
            ),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            state
                .tokens
                .list()
                .iter()
                .find(|t| t.name == "Pixel")
                .unwrap()
                .last_from
                .as_deref(),
            Some("raven@github")
        );
        state.tokens.revoke(&ro_tok.id).unwrap();
        assert_eq!(
            status(call("GET", "/api/roots", Some(&ro), None).await.unwrap()),
            StatusCode::UNAUTHORIZED
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn share_links_serve_files_drop_boxes_and_passwords() {
        let dir = std::env::temp_dir().join(format!("holodrive-web-links-{}", auth::random_hex(4)));
        std::fs::create_dir_all(dir.join("a/inbox")).unwrap();
        std::fs::write(dir.join("a/inbox/secret.txt"), b"hidden").unwrap();
        std::fs::write(dir.join("a/song.mp3"), b"la la").unwrap();
        let mut cfg = Config::starter();
        cfg.password_hash = "configured".into();
        cfg.hosts.push("blackbox.tail9311b5.ts.net:8768".into());
        cfg.roots = vec![crate::config::Root {
            id: "a".into(),
            name: "A".into(),
            path: dir.join("a"),
            icon: String::new(),
            writable: true,
        }];
        let state = Arc::new(AppState {
            cfg: Arc::new(cfg),
            auth: Auth::new(30),
            port: 8768,
            thumbs: tokio::sync::Semaphore::new(2),
            stopping: tokio::sync::watch::channel(false).1,
            themes: tokio::sync::watch::channel(None).1,
            sending: Default::default(),
            tokens: crate::tokens::Store::memory(),
            links: crate::links::Store::memory(),
        });
        let new = |path: &str, mode| crate::links::NewLink {
            path: path.into(),
            mode,
            days: None,
            password: None,
            max_downloads: None,
            label: None,
        };
        let song = state
            .links
            .create(
                &state.cfg,
                crate::links::NewLink {
                    max_downloads: Some(1),
                    ..new("a/song.mp3", crate::links::Mode::View)
                },
            )
            .unwrap();
        let drop = state
            .links
            .create(
                &state.cfg,
                crate::links::NewLink {
                    password: Some("pw".into()),
                    ..new("a/inbox", crate::links::Mode::Drop)
                },
            )
            .unwrap();
        let app = router(state.clone());
        // Nobody signed in: a stranger on the tailnet with only the link.
        let call = |method: &str, uri: String, cookie: Option<String>, body: Body| {
            let mut b = Request::builder()
                .method(method)
                .uri(uri)
                .header(header::HOST, "blackbox.tail9311b5.ts.net:8768")
                .header("tailscale-user-login", "friend@github");
            if let Some(c) = cookie {
                b = b.header(header::COOKIE, c);
            }
            app.clone().oneshot(b.body(body).unwrap())
        };
        let get = |uri: String| call("GET", uri, None, Body::empty());

        assert_eq!(
            get(format!("/s/{}", song.key)).await.unwrap().status(),
            StatusCode::OK
        );
        let r = get(format!("/api/s/{}/raw?dl=1", song.key)).await.unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(
            &axum::body::to_bytes(r.into_body(), 64).await.unwrap()[..],
            b"la la"
        );
        assert_eq!(
            get(format!("/api/s/{}/raw?dl=1", song.key))
                .await
                .unwrap()
                .status(),
            StatusCode::GONE
        );
        assert_eq!(
            get(format!("/api/s/{}/raw?p=../x", song.key))
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            get(format!("/api/s/{}", "0".repeat(32)))
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
        // The rest of the app stays closed.
        assert_eq!(
            get("/api/list?path=a".into()).await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );

        // Drop box: locked until the password; then upload but never list.
        let up = |cookie| {
            call(
                "POST",
                format!("/api/s/{}/upload?name=hi.txt", drop.key),
                cookie,
                Body::from("hey"),
            )
        };
        assert_eq!(up(None).await.unwrap().status(), StatusCode::UNAUTHORIZED);
        let unlock = |pw: &str| {
            app.clone().oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/s/{}/unlock", drop.key))
                    .header(header::HOST, "blackbox.tail9311b5.ts.net:8768")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(json!({ "password": pw }).to_string()))
                    .unwrap(),
            )
        };
        assert_eq!(
            unlock("nope").await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
        let ok = unlock("pw").await.unwrap();
        assert_eq!(ok.status(), StatusCode::OK);
        let set = ok.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .to_string();
        assert!(set.contains(&format!("Path=/api/s/{}", drop.key)) && set.contains("HttpOnly"));
        let cookie = set.split(';').next().unwrap().to_string();
        assert_eq!(
            up(Some(cookie.clone())).await.unwrap().status(),
            StatusCode::OK
        );
        assert_eq!(std::fs::read(dir.join("a/inbox/hi.txt")).unwrap(), b"hey");
        let listed = call(
            "GET",
            format!("/api/s/{}/list", drop.key),
            Some(cookie.clone()),
            Body::empty(),
        );
        assert_eq!(listed.await.unwrap().status(), StatusCode::FORBIDDEN);
        assert_eq!(state.links.get(&drop.key).unwrap().uploads, 1);

        state.links.revoke(&drop.key).unwrap();
        assert_eq!(
            up(Some(cookie)).await.unwrap().status(),
            StatusCode::NOT_FOUND
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn theme_events_push_changes_and_end_when_the_server_stops() {
        let mut cfg = Config::starter();
        cfg.password_hash = "configured".into();
        cfg.local_auth_bypass = true;
        let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
        let (theme_tx, theme_rx) = tokio::sync::watch::channel(Some(7));
        let state = Arc::new(AppState {
            cfg: Arc::new(cfg),
            auth: Auth::new(30),
            port: 8768,
            thumbs: tokio::sync::Semaphore::new(2),
            stopping: stop_rx,
            themes: theme_rx,
            sending: Default::default(),
            tokens: crate::tokens::Store::memory(),
            links: crate::links::Store::memory(),
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
        let wait = std::time::Duration::from_secs(2);
        let mut next_text = async || {
            let chunk = tokio::time::timeout(wait, body.next()).await.ok()??.ok()?;
            Some(String::from_utf8_lossy(&chunk).into_owned())
        };
        assert!(
            next_text().await.unwrap().contains("data: 7"),
            "current revision on connect"
        );
        theme_tx.send(Some(8)).unwrap();
        assert!(
            next_text().await.unwrap().contains("data: 8"),
            "changes are pushed"
        );
        stop_tx.send(true).unwrap();
        let end = tokio::time::timeout(wait, body.next()).await;
        assert!(end.expect("stream still open after shutdown").is_none());
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
            themes: tokio::sync::watch::channel(None).1,
            sending: Default::default(),
            tokens: crate::tokens::Store::memory(),
            links: crate::links::Store::memory(),
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
