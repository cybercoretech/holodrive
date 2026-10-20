//! Narrow same-user IPC client for CyberVault's append-only local bridge.

use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;
use tokio::task;
use zeroize::Zeroizing;

const MAX_RESPONSE: usize = 4096;
const TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Serialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum BridgeRequest<'a> {
    Status,
    Store { label: &'a str, secret: &'a str },
}

#[derive(Deserialize)]
struct BridgeReply {
    api_version: String,
    ok: bool,
    unlocked: bool,
    label: Option<String>,
    error: Option<String>,
}

#[derive(Serialize)]
pub struct Status {
    pub api_version: &'static str,
    pub available: bool,
    pub unlocked: bool,
    pub reason: Option<&'static str>,
}

#[derive(Deserialize)]
pub struct StoreRequest {
    pub label: String,
    pub secret: Zeroizing<String>,
}

#[derive(Serialize)]
pub struct StoreReply {
    pub api_version: &'static str,
    pub stored: bool,
    pub label: Option<String>,
    pub reason: Option<String>,
}

fn socket_path() -> PathBuf {
    if let Some(path) = std::env::var_os("CYBERVAULT_BRIDGE_SOCKET") {
        return PathBuf::from(path);
    }
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
        .join(".local/state/cybervault/cybervault.sock")
}

pub async fn status() -> Status {
    match task::spawn_blocking(|| query(&BridgeRequest::Status)).await {
        Ok(Ok(reply)) if reply.api_version == "v1" => Status {
            api_version: "status/v1",
            available: true,
            unlocked: reply.unlocked,
            reason: None,
        },
        _ => Status {
            api_version: "status/v1",
            available: false,
            unlocked: false,
            reason: Some("bridge_unavailable"),
        },
    }
}

pub async fn store(body: StoreRequest) -> Result<StoreReply, &'static str> {
    if body.label.trim().is_empty()
        || body.label.len() > 128
        || body.label.chars().any(char::is_control)
        || body.secret.is_empty()
        || body.secret.len() > 16 * 1024
    {
        return Err("invalid_payload");
    }
    let result = task::spawn_blocking(move || {
        query(&BridgeRequest::Store {
            label: body.label.trim(),
            secret: body.secret.as_str(),
        })
    })
    .await
    .map_err(|_| "bridge_unavailable")?
    .map_err(|_| "bridge_unavailable")?;
    if result.api_version != "v1" {
        return Err("unsupported_bridge_version");
    }
    Ok(StoreReply {
        api_version: "store/v1",
        stored: result.ok,
        label: result.label,
        reason: result.error,
    })
}

fn query(request: &BridgeRequest<'_>) -> Result<BridgeReply, &'static str> {
    let mut stream = UnixStream::connect(socket_path()).map_err(|_| "bridge_unavailable")?;
    stream
        .set_read_timeout(Some(TIMEOUT))
        .map_err(|_| "bridge_unavailable")?;
    stream
        .set_write_timeout(Some(TIMEOUT))
        .map_err(|_| "bridge_unavailable")?;
    let bytes = Zeroizing::new(serde_json::to_vec(request).map_err(|_| "invalid_request")?);
    stream.write_all(&bytes).map_err(|_| "bridge_unavailable")?;
    stream
        .shutdown(std::net::Shutdown::Write)
        .map_err(|_| "bridge_unavailable")?;
    let mut response = Vec::new();
    stream
        .take((MAX_RESPONSE + 1) as u64)
        .read_to_end(&mut response)
        .map_err(|_| "bridge_unavailable")?;
    if response.len() > MAX_RESPONSE {
        return Err("response_too_large");
    }
    serde_json::from_slice(&response).map_err(|_| "invalid_response")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn rejects_invalid_store_payloads_before_touching_the_socket() {
        let result = store(StoreRequest {
            label: "\n".into(),
            secret: Zeroizing::new("secret".into()),
        })
        .await;
        assert_eq!(result.err().unwrap(), "invalid_payload");
        let result = store(StoreRequest {
            label: "valid".into(),
            secret: Zeroizing::new(String::new()),
        })
        .await;
        assert_eq!(result.err().unwrap(), "invalid_payload");
    }
}
