//! Holodrive — a neon web file manager for your own machine.
//!
//! Serves on 127.0.0.1 (default :8768); `tailscale serve` publishes it to
//! the tailnet. Config: ~/.config/holodrive/config.toml.

mod auth;
mod config;
mod cybergrid;
mod fsops;
mod jsonfile;
mod links;
mod media;
mod samba;
mod smbconf;
mod snapshots;
mod tokens;
mod uploads;
mod views;
mod web;

use config::Config;
use std::path::PathBuf;
use std::sync::Arc;

fn usage() -> ! {
    println!(
        "holodrive — neon web file manager (Cybercore Tech)\n\n\
         USAGE:\n  \
         holodrive [--config PATH]          serve (default)\n  \
         holodrive init [--host NAME:PORT]  write a starter config with your shared folders\n  \
         holodrive passwd                   set the login password\n  \
         holodrive token add NAME [--read-only]  make a device token (shown once)\n  \
         holodrive token list               show device tokens\n  \
         holodrive token revoke ID          revoke one\n\n\
         Config: {}",
        config::config_path().display()
    );
    std::process::exit(0)
}

fn die(msg: impl std::fmt::Display) -> ! {
    eprintln!("holodrive: {msg}");
    std::process::exit(1)
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut config_path = config::config_path();
    if let Some(i) = args.iter().position(|a| a == "--config") {
        if i + 1 >= args.len() {
            usage()
        }
        config_path = PathBuf::from(args.remove(i + 1));
        args.remove(i);
    }
    match args.first().map(String::as_str) {
        None => serve(config_path),
        Some("init") => init(config_path, &args[1..]),
        Some("passwd") => passwd(config_path),
        Some("token") => token(config_path, &args[1..]),
        Some("-h" | "--help" | "help") => usage(),
        Some(other) => die(format!("unknown command {other:?} (try --help)")),
    }
}

fn init(path: PathBuf, rest: &[String]) {
    if path.exists() {
        die(format!(
            "{} already exists; edit it instead",
            path.display()
        ));
    }
    let mut cfg = Config::starter();
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--host" => cfg
                .hosts
                .push(it.next().cloned().unwrap_or_else(|| usage())),
            other => die(format!("unknown init option {other:?}")),
        }
    }
    cfg.save(&path).unwrap_or_else(|e| die(e));
    println!("wrote {} with {} folders:", path.display(), cfg.roots.len());
    for r in &cfg.roots {
        println!(
            "  {} {:<11} {}{}",
            r.icon,
            r.name,
            r.path.display(),
            if r.writable { "" } else { "  (read-only)" }
        );
    }
    println!("next: holodrive passwd");
}

fn passwd(path: PathBuf) {
    let mut cfg = Config::load(&path).unwrap_or_else(|e| die(e));
    let p1 = rpassword::prompt_password("new Holodrive password: ").unwrap_or_else(|e| die(e));
    if p1.chars().count() < 12 {
        die("use at least 12 characters");
    }
    let p2 = rpassword::prompt_password("again: ").unwrap_or_else(|e| die(e));
    if p1 != p2 {
        die("they didn't match");
    }
    cfg.password_hash = auth::hash_password(&p1).unwrap_or_else(|e| die(e));
    cfg.save(&path).unwrap_or_else(|e| die(e));
    println!("password saved; restart Holodrive to use it (systemctl --user restart holodrive)");
}

const SHUTDOWN_GRACE: std::time::Duration = std::time::Duration::from_secs(10);

fn token(config: PathBuf, rest: &[String]) {
    let store = tokens::Store::open(tokens::path_for(&config));
    let when = |t: Option<u64>| {
        let Some(t) = t else {
            return "never".to_string();
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(t);
        match now.saturating_sub(t) {
            s if s < 60 => "just now".into(),
            s if s < 3600 => format!("{} min ago", s / 60),
            s if s < 86_400 => format!("{} h ago", s / 3600),
            s => format!("{} days ago", s / 86_400),
        }
    };
    match rest.first().map(String::as_str) {
        Some("add") => {
            let read_only = rest.iter().any(|a| a == "--read-only");
            let name: Vec<&str> = rest[1..]
                .iter()
                .filter(|a| *a != "--read-only")
                .map(String::as_str)
                .collect();
            let (t, secret) = store
                .create(&name.join(" "), read_only)
                .unwrap_or_else(|e| die(e));
            println!(
                "token for {:?} ({}), id {}:\n\n  {secret}\n\nShown once; it's not stored. Revoke with `holodrive token revoke {}`.",
                t.name,
                if t.read_only {
                    "read-only"
                } else {
                    "full access"
                },
                t.id,
                t.id
            );
        }
        Some("list") => {
            for t in store.list() {
                println!(
                    "{}  {:<24} {:<11} last used {}{}",
                    t.id,
                    t.name,
                    if t.read_only { "read-only" } else { "full" },
                    when(t.last_used),
                    t.last_from
                        .map(|f| format!(" from {f}"))
                        .unwrap_or_default()
                );
            }
        }
        Some("revoke") => {
            let id = rest
                .get(1)
                .unwrap_or_else(|| die("which id? see `holodrive token list`"));
            match store.revoke(id) {
                Ok(true) => println!("revoked {id}"),
                Ok(false) => die(format!("no token {id}")),
                Err(e) => die(e),
            }
        }
        _ => die("usage: holodrive token add NAME [--read-only] | list | revoke ID"),
    }
}

#[tokio::main]
async fn serve(path: PathBuf) {
    let cfg = Config::load(&path).unwrap_or_else(|e| die(format!("{e:#}")));
    if cfg.password_hash.is_empty() {
        eprintln!(
            "holodrive: no password yet — run `holodrive passwd`; logins are refused until then"
        );
    }
    media::clean_tmp();
    let purged = fsops::purge_old(&cfg, cfg.trash_days);
    if purged > 0 {
        println!(
            "[Holodrive] purged {purged} trash item(s) older than {} days",
            cfg.trash_days
        );
    }
    let swept = uploads::sweep(&cfg, true);
    if swept > 0 {
        println!("[Holodrive] removed {swept} unfinished upload(s)");
    }
    let listen = cfg.listen.clone();
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let state = Arc::new(web::AppState {
        port: cfg.port(),
        auth: auth::Auth::new(cfg.session_days),
        cfg: Arc::new(cfg),
        thumbs: tokio::sync::Semaphore::new(2),
        stopping: stop_rx.clone(),
        sending: Default::default(),
        tokens: tokens::Store::open(tokens::path_for(&path)),
        links: links::Store::open(links::path_for(&path)),
    });
    let daily = state.cfg.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(86_400)).await;
            let cfg = daily.clone();
            let _ = tokio::task::spawn_blocking(move || {
                fsops::purge_old(&cfg, cfg.trash_days);
                uploads::sweep(&cfg, false);
            })
            .await;
        }
    });
    let app = web::router(state);
    let listener = tokio::net::TcpListener::bind(&listen)
        .await
        .unwrap_or_else(|e| die(format!("can't bind {listen}: {e}")));
    println!("[Holodrive] listening on http://{listen}/");
    let shutdown = async move {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("signal");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
        let _ = stop_tx.send(true);
    };
    let server = axum::serve(listener, app).with_graceful_shutdown(shutdown);
    // Give in-flight requests (downloads, video streams, uploads) a moment to
    // finish, but never hang the stop: systemd would SIGKILL us after 90 s.
    let mut stopping = stop_rx;
    let grace = async move {
        let _ = stopping.wait_for(|stop| *stop).await;
        tokio::time::sleep(SHUTDOWN_GRACE).await;
    };
    tokio::select! {
        r = std::future::IntoFuture::into_future(server) => r.expect("server"),
        _ = grace => eprintln!(
            "[Holodrive] requests still open after {} s; stopping anyway",
            SHUTDOWN_GRACE.as_secs()
        ),
    }
}
