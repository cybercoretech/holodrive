# GhostPort status integration

HOLODRIVE exposes GhostPort's read-only connection health through its own
authenticated API:

```text
GET /api/integrations/ghostport/status
```

The endpoint is protected by HOLODRIVE login or an authorized device token. It
does not accept tunnel-control requests and never makes GhostPort's Unix socket
network-accessible. The HOLODRIVE server reads the socket locally, so the two
daemons must run as the same OS user (GhostPort creates the socket with mode
`0600`). For a nonstandard socket path, set `GHOSTPORT_STATUS_SOCKET` in the
HOLODRIVE server's service environment.

Example from a client that has a HOLODRIVE device token:

```sh
curl -fsS -H "Authorization: Bearer $HOLODRIVE_TOKEN" \
  https://holodrive.example/api/integrations/ghostport/status
```

When GhostPort is running, the response has `api_version: "status/v1"`,
`available: true`, and a `snapshot` using GhostPort's versioned status IPC v1
schema. HOLODRIVE removes the peer address and peer name from the snapshot
before returning it to clients. When the daemon is stopped or its socket cannot be reached, the API
returns `available: false` with a stable reason such as `daemon_unavailable`,
`timeout`, or `invalid_response`. The API intentionally has no start, stop,
configuration, identity, or forwarding controls.

This supports a secure connection-health display in the web and Android apps.
The actual phone data path remains HTTPS to HOLODRIVE, optionally carried over
a GhostPort reverse link; it does not send GhostPort private keys or config to
the phone.
