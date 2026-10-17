# CyberVault share-secret integration

HOLODRIVE can append a newly created share URL and its passphrase to CyberVault. This keeps the passphrase recoverable without putting it in HOLODRIVE's link database, logs, or a public API response.

## Enable the local bridge

Install CyberVault on the HOLODRIVE server and run:

```sh
cybervault bridge
```

Enter the master password in that local terminal. The command exposes a same-user Unix socket at `~/.local/state/cybervault/cybervault.sock`, creates its parent with mode `0700`, protects the socket with mode `0600`, and exits after five idle minutes. Set `CYBERVAULT_BRIDGE_SOCKET` for a nonstandard socket location. HOLODRIVE and CyberVault must run as the same OS user.

## Save a share

After creating a protected link, choose **Save to CyberVault** in the web app, or **Save link + passphrase to CyberVault** in Android. The stored secret contains the share URL and passphrase. Choose a distinct entry label; CyberVault refuses to overwrite existing labels.

The endpoint is `POST /api/integrations/cybervault/store`. It requires an interactive HOLODRIVE password session and CSRF token. Device-token authentication is explicitly denied, including write-enabled tokens. The endpoint does not return the secret. A browser or phone can submit a share only while the local bridge is running and unlocked.

The bridge protocol supports only `status` and append-only `store`; it has no remote unlock, secret retrieval, list, overwrite, or delete action. The request is limited to a 128-byte label and 16 KiB secret. Submitted buffers are zeroized after use. TLS or the private Tailscale path still protects the HOLODRIVE connection from the phone.
