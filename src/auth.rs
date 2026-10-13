//! Password login, sessions and the brute-force brake.
//!
//! One password (argon2id hash in the config). A login hands out a random
//! session id in an HttpOnly, SameSite=Strict cookie plus a per-session CSRF
//! token the page sends back on every state-changing request. Sessions live
//! in memory, so restarting Holodrive signs everyone out.
//!
//! Everything arrives through `tailscale serve` from 127.0.0.1, so the
//! brake is global: 5 wrong passwords in 10 minutes and logins pause until
//! the oldest failure ages out.

use argon2::Argon2;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use std::collections::{HashMap, VecDeque};
use std::io::Read;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const COOKIE: &str = "holodrive_session";
const MAX_FAILS: usize = 5;
const FAIL_WINDOW: Duration = Duration::from_secs(600);

pub fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut buf))
        .expect("read /dev/urandom");
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn hash_password(password: &str) -> anyhow::Result<String> {
    let mut raw = [0u8; 16];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut raw)?;
    let salt = SaltString::encode_b64(&raw).map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .to_string())
}

pub fn verify_password(hash: &str, password: &str) -> bool {
    PasswordHash::new(hash)
        .map(|h| {
            Argon2::default()
                .verify_password(password.as_bytes(), &h)
                .is_ok()
        })
        .unwrap_or(false)
}

#[derive(Clone)]
pub struct Session {
    pub csrf: String,
    pub expires: Instant,
}

pub struct Auth {
    sessions: Mutex<HashMap<String, Session>>,
    fails: Mutex<VecDeque<Instant>>,
    pub lifetime: Duration,
}

pub enum LoginBrake {
    Open,
    Wait(Duration),
}

impl Auth {
    pub fn new(days: u64) -> Auth {
        Auth {
            sessions: Mutex::new(HashMap::new()),
            fails: Mutex::new(VecDeque::new()),
            lifetime: Duration::from_secs(days.max(1) * 86_400),
        }
    }

    pub fn brake(&self) -> LoginBrake {
        let mut fails = self.fails.lock().unwrap();
        while fails.front().is_some_and(|t| t.elapsed() > FAIL_WINDOW) {
            fails.pop_front();
        }
        if fails.len() >= MAX_FAILS {
            let oldest = *fails.front().unwrap();
            LoginBrake::Wait(FAIL_WINDOW.saturating_sub(oldest.elapsed()))
        } else {
            LoginBrake::Open
        }
    }

    pub fn record_fail(&self) {
        self.fails.lock().unwrap().push_back(Instant::now());
    }

    pub fn create(&self) -> (String, Session) {
        let id = random_hex(32);
        let session = Session {
            csrf: random_hex(24),
            expires: Instant::now() + self.lifetime,
        };
        let mut map = self.sessions.lock().unwrap();
        map.retain(|_, s| s.expires > Instant::now());
        map.insert(id.clone(), session.clone());
        (id, session)
    }

    pub fn get(&self, id: &str) -> Option<Session> {
        let map = self.sessions.lock().unwrap();
        map.get(id).filter(|s| s.expires > Instant::now()).cloned()
    }

    pub fn remove(&self, id: &str) {
        self.sessions.lock().unwrap().remove(id);
    }
}

pub fn cookie_value<'a>(header: &'a str, name: &str) -> Option<&'a str> {
    header.split(';').find_map(|part| {
        let (k, v) = part.trim().split_once('=')?;
        (k == name).then_some(v)
    })
}

pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_roundtrip() {
        let h = hash_password("correct horse battery").unwrap();
        assert!(h.starts_with("$argon2id$"));
        assert!(verify_password(&h, "correct horse battery"));
        assert!(!verify_password(&h, "wrong"));
        assert!(!verify_password("garbage", "x"));
    }

    #[test]
    fn brake_after_five() {
        let a = Auth::new(1);
        for _ in 0..4 {
            a.record_fail();
            assert!(matches!(a.brake(), LoginBrake::Open));
        }
        a.record_fail();
        assert!(matches!(a.brake(), LoginBrake::Wait(_)));
    }

    #[test]
    fn cookies() {
        assert_eq!(
            cookie_value("a=1; holodrive_session=abc; b=2", COOKIE),
            Some("abc")
        );
        assert_eq!(cookie_value("a=1", COOKIE), None);
    }
}
