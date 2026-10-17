//! Read-only bridge to GhostPort's owner-only Unix status socket.
//!
//! This intentionally exposes status only. GhostPort configuration,
//! identities, and tunnel controls remain owned by GhostPort itself.

use serde::Serialize;
use serde_json::Value;
use std::path::PathBuf;
use tokio::io::AsyncReadExt;
use tokio::net::UnixStream;

const MAX_STATUS_BYTES: usize = 64 * 1024;
const QUERY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

#[derive(Debug, Serialize)]
pub struct IntegrationStatus {
    pub api_version: &'static str,
    pub available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<&'static str>,
}

impl IntegrationStatus {
    fn unavailable(reason: &'static str) -> Self {
        Self {
            api_version: "status/v1",
            available: false,
            snapshot: None,
            reason: Some(reason),
        }
    }
}

/// Resolve the standard GhostPort IPC socket, with an override for
/// installations that run the daemon under a dedicated service account.
pub fn socket_path() -> PathBuf {
    if let Some(path) = std::env::var_os("GHOSTPORT_STATUS_SOCKET") {
        return PathBuf::from(path);
    }
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
        .join(".local/state/ghostport/ghostport.sock")
}

pub async fn status() -> IntegrationStatus {
    match tokio::time::timeout(QUERY_TIMEOUT, query(&socket_path())).await {
        Err(_) => IntegrationStatus::unavailable("timeout"),
        Ok(Err(reason)) => IntegrationStatus::unavailable(reason),
        Ok(Ok(snapshot)) => IntegrationStatus {
            api_version: "status/v1",
            available: true,
            snapshot: Some(snapshot),
            reason: None,
        },
    }
}

async fn query(path: &std::path::Path) -> Result<Value, &'static str> {
    let socket = UnixStream::connect(path)
        .await
        .map_err(|_| "daemon_unavailable")?;
    let mut bytes = Vec::new();
    socket
        .take((MAX_STATUS_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| "invalid_response")?;
    if bytes.len() > MAX_STATUS_BYTES {
        return Err("response_too_large");
    }
    let mut value = decode_status(&bytes)?;
    redact_peer_details(&mut value);
    Ok(value)
}

fn redact_peer_details(value: &mut Value) {
    // These are useful to a local GhostPort operator but are not needed by
    // HOLODRIVE clients; omit them from device-token replies.
    if let Some(snapshot) = value.as_object_mut() {
        snapshot.remove("control_peer_addr");
        snapshot.remove("control_peer_name");
    }
}

fn decode_status(bytes: &[u8]) -> Result<Value, &'static str> {
    if bytes.len() > MAX_STATUS_BYTES {
        return Err("response_too_large");
    }
    let value: Value = serde_json::from_slice(bytes).map_err(|_| "invalid_response")?;
    let object = value.as_object().ok_or("invalid_response")?;
    if !object.get("role").is_some_and(Value::is_string)
        || !object
            .get("control_connected")
            .is_some_and(Value::is_boolean)
        || !object.get("links").is_some_and(Value::is_array)
    {
        return Err("invalid_response");
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn reports_unavailable_when_socket_is_missing() {
        let path = std::env::temp_dir().join(format!(
            "holodrive-ghostport-missing-{}.sock",
            std::process::id()
        ));
        assert_eq!(query(&path).await.unwrap_err(), "daemon_unavailable");
    }

    #[test]
    fn decodes_a_valid_status_snapshot() {
        let mut snapshot = decode_status(
            br#"{"role":"client","control_connected":true,"links":[],"control_peer_addr":"192.0.2.1:10","control_peer_name":"home"}"#,
        )
        .unwrap();
        redact_peer_details(&mut snapshot);
        assert_eq!(snapshot["role"], "client");
        assert!(snapshot.get("control_peer_addr").is_none());
        assert!(snapshot.get("control_peer_name").is_none());
    }

    #[test]
    fn rejects_non_status_json_and_oversize_payloads() {
        assert_eq!(
            decode_status(br#"{"secret":"no"}"#).unwrap_err(),
            "invalid_response"
        );
        assert_eq!(
            decode_status(&vec![b' '; MAX_STATUS_BYTES + 1]).unwrap_err(),
            "response_too_large"
        );
    }
}
