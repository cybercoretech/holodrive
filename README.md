<p align="center"><img src="static/holocube.svg" width="96" alt=""></p>

# 🧊 HOLODRIVE

Project page: <https://cybercoretech.github.io/holodrive/>.

**A neon web file manager for your own machine, reached over your tailnet.**
Browse, preview, upload, move and restore the folders you share — from the
desktop, another box, or your phone — in the same CYBERGRID look as the rest of
Cybercore. One Rust binary, no database, no Node.

## ✨ What it does

- **Drives.** Each shared folder is a drive in the sidebar, with free space and
  a read-only badge where it applies (e.g. ISOs).
- **Grid or list.** Glowing holographic folders tinted per name, emoji tags for
  known folders (📱 `from-phone`, 🤝 `handoffs`, 🧬 `src`…), file-type emoji,
  and real thumbnails for pictures and videos.
- **Photos library.** Open Photos in the sidebar to browse raster images across
  a drive, grouped by year, without changing where files are stored. Select a
  drive, open images in the existing lightbox, and download selections. Scans
  use JPEG EXIF capture dates when available (otherwise modified time), skip
  hidden content and links, and stop at a fixed safety limit on very large
  drives.
- **Live folders and phone handoff.** Open folders refresh themselves when
  files arrive through Samba or another local app. From a folder, **📱 Send to
  phone** shows a QR code and copies a `holodrive://` folder link; Holodrive Photos opens that
  folder on the phone after sign-in. Recent folder changes, uploads, and sync
  summaries appear in the shared Activity view on web and Android. The Android
  app refreshes its visible
  folder while it is in the foreground.
- **Lightbox.** Pictures with zoom (wheel, double-click, pinch), drag to pan,
  swipe or ←/→ between them, and a filmstrip. Videos stream with seeking; audio
  plays on a spinning disc; PDFs open inline.
- **Read anything textual.** Code with line numbers and highlighting, and
  Markdown rendered safely (no raw HTML).
- **Upload anywhere.** Drag files or whole folders onto the page, paste
  (Ctrl+V a screenshot), or open the upload panel (its own neon drop zone, a
  staged list and a destination picker; the browser's file chooser only opens
  if you ask for it). Parallel uploads with live progress; nothing lands until
  it's complete; names never clobber (`photo (2).jpg`). Files over 8 MB go up
  in **resumable chunks**: a dropped connection or a server restart just
  retries from where it got to, and after a reload, uploading the same file
  again picks up where it stopped. Unfinished uploads are cleared after a day.
- **Edit in place.** A CodeMirror 6 editor with **Vim keys** (`:w`, `:q`,
  `:wq`, `:x`, `jk` = Esc; toggle off for plain keys), syntax highlighting for
  ~30 languages in your theme's colours, search, folding and Ctrl+S. Saves are
  atomic and refuse to overwrite a file changed elsewhere unless you say so.
  Loaded only the first time you edit (~390 KB gzipped, cached).
- **Move fast.** Multi-select (click, Ctrl, Shift, long-press on touch),
  drag onto folders, breadcrumbs or drives to move (Ctrl/Alt = copy),
  cut/copy/paste, rename, duplicate, Copy to…/Move to… with a folder picker,
  and zip downloads of folders or selections.
- **Nothing is lost by accident.** Delete moves items to a per-drive trash
  (instant, on the same filesystem) with **Undo**; the Trash view restores or
  purges; old trash goes after 30 days.
- **Samba panel.** See smbd's state, every share (read-write/read-only,
  hidden, disabled, who's connected) and every connected client with its
  protocol and encryption. Add a share from any folder (right-click →
  *Share via Samba…*), edit, rename, make read-only, disable, disconnect
  everyone, remove, reload or restart smbd, and roll back to any of the last
  20 `smb.conf` versions. See *Samba helper* below.
- **smb.conf editor, two ways** (Samba page): **🧩 GUI** — every section as
  a card, every setting as a row (toggles for yes/no, a folder picker for
  paths, suggestions for common settings), comments and order kept, risky
  settings flagged as you type — or **⌨️ Vim** — the whole file in the
  editor. Switch between them without losing edits. Both save through the
  helper with the same rails (see below).
- **Command line** (`:`), Vim style: `:mkdir a/b/c`, `:touch notes.md`,
  `:e file` (made if missing), `:cd ..` / `:cd /shared/x`, `:rm`, `:mv`,
  `:cp`, `:find`, `:share`, `:time`, `:theme <name>`, `:samba`, `:upload`,
  `:help`. Tab completes names, ↑/↓ recalls history, quotes for spaces.
- **Time machine.** For drives covered by snapper, press ⏳ (or `t`) to see
  any folder as it was in any snapshot: a timeline of every snapshot, `[`/`]`
  to step, files deleted since marked ✗, changed ones ±. Restore items in
  place (the current ones go to the trash first, so it's undoable) or as
  copies. Every file has **Versions…** (`v`): each distinct past version with
  its size change, to preview, download or restore. Read-only access through
  snapper's `ALLOW_USERS` + `SYNC_ACL`; no root needed.
- **Search** a drive by name.
- **Properties** show permissions, the real path, and the Samba path
  (`\\blackbox\Shared\…`, `smb://…`) to copy.
- **70+ themes.** The theme picker uses the shared Cybercore catalog. It's per
  device, so picking a theme on the phone doesn't repaint the desktop; until you
  pick one, it follows the desktop's active theme live.
- **Keyboard.** Arrows, Enter, Space (preview), Backspace (up), Delete, F2,
  Ctrl+A/C/X/V, `/` search, `g`/`l` grid/list, `u` upload, `n` new folder,
  `:` command line, `e` edit, `v` versions, `t` time machine, `i` properties, `r` refresh.

## 🛡️ Security model

- Listens on **127.0.0.1** only. `tailscale serve` publishes it to your tailnet
  (WireGuard-encrypted); nothing is opened to the LAN or the internet.
- **One password**, stored as an argon2id hash (apps and scripts use
  revocable device tokens instead; see *Devices*). A login sets an HttpOnly,
  SameSite=Strict session cookie; every change also needs the session's CSRF
  token and an allowed Origin. Only known `Host` names are accepted (DNS
  rebinding). Five wrong passwords in ten minutes pause logins.
- Optional `local_auth_bypass = true` skips login only for direct
  `127.0.0.1` / `localhost` access. It is disabled by default and does not
  affect the Tailscale host or remote clients.
- **Paths can't escape.** Every path is resolved against its drive; `..`,
  hidden names (dot files: `.git`, `.env`, `.snapshots`) and symlinks that lead
  outside the drive are refused. Read-only drives refuse every change.
- **Files can't attack the page.** Raw files are served with
  `Content-Security-Policy: sandbox`; HTML and SVG download instead of opening.
  The app itself runs under a strict CSP with no inline script.

## 📦 Install

```sh
./packaging/install.sh            # builds, writes ~/.config/holodrive/config.toml,
                                  # asks for a password, starts holodrive.service,
                                  # and runs `tailscale serve` for you
```

The first run offers the folders it finds (Shared, Devspace, Webdev,
Animations, Documents, Pictures, Downloads, Wiki, ISOs). Edit the config to
change them:

```toml
listen = "127.0.0.1:8768"
hosts = ["blackbox.tail1234.ts.net:8768"]   # Host names tailscale serve forwards
password_hash = "$argon2id$…"               # set with `holodrive passwd`
local_auth_bypass = false                   # true trusts this PC's localhost
session_days = 30
trash_days = 30

[[roots]]
id = "shared"          # URL id
name = "Shared"        # shown in the sidebar (and used for the Samba path)
path = "/home/you/Shared"
icon = "🛰️"
writable = true
```

Then `systemctl --user restart holodrive`. Thumbnails cache in
`~/.cache/holodrive`. Optional: `ffmpeg` for video thumbnails.

## 📱 Android app

Version 0.1.0 of the signed companion app is available for Android 11 and newer. [Download the APK](https://github.com/cybercoretech/holodrive-android-app/releases/latest/download/holodrive-photos.apk), read the [Android install and feature guide](https://cybercoretech.github.io/holodrive-android-app/), or see the [release notes and SHA-256 checksum](https://github.com/cybercoretech/holodrive-android-app/releases/latest). For update notifications, add `https://github.com/cybercoretech/holodrive-android-app` to [Obtainium](https://github.com/ImranR98/Obtainium).

## 🧩 Samba helper (optional)

The share list works without privileges (smb.conf is world-readable).
Changing it, seeing sessions and controlling smbd go through a small root
helper, `holodrive-sambactl`, run with `pkexec`:

```sh
sudo ./packaging/install-root.sh            # after ./packaging/install.sh
sudo ./packaging/install-root.sh --uninstall
```

A polkit rule lets only the user who ran it use the helper without a
password (Holodrive is a service and can't ask). The helper trusts nothing
it's given:

- it edits **share sections only**: name, path, comment, read only,
  browseable, available. `[global]` (encryption, allowed hosts…) and any
  other per-share options are kept exactly as they are;
- a share must be an existing folder **inside your home**, not the home
  itself, with no hidden component, not world-writable, and not under
  anything in `/etc/holodrive/samba-deny.list` (Vault, Personal, MEGA… —
  root-owned, edit with sudo);
- whole-file saves (GUI and Vim) also pass **safety rails**: nothing that
  runs programs or reads other files (`*command*`, `*script*`, `*exec*`,
  `include`, `config file`, `magic script`, `printcap name` other than
  `/dev/null`…), no `force user`/`force group`/`admin users`/`username map`,
  no guest access (`map to guest` must be `never`), no `wide links`, no
  `[homes]`/`[printers]`, log/lock/state paths only under Samba's own
  directories, no line continuations; weaker settings (encryption not
  required, pre-SMB3 clients, no `hosts allow`) need a confirmation; and a
  save is refused if the file changed since the editor opened it;
- every new smb.conf passes `testparm` before it replaces the old one, which
  is backed up to `/var/lib/holodrive/smb-backups` (last 20 kept);
- every change is logged: `journalctl -t holodrive-sambactl`.

## 🔗 Share links

Give someone one file or folder, or a **drop box** to upload into, without
the Holodrive password: right-click → **Share link…** (or `:link` in a
folder). Choose an expiry (1 day to never), an optional password, a download
limit and the title they see. They get `http://<tailnet host>/s/<key>`, a
small page to preview, browse and download (single files or a zip), or, for
a drop box, to upload files without seeing what's already there.

- Keys are 128 random bits; links live in `~/.config/holodrive/links.json`
  (0600), so 🔗 **Share links** can show and copy them again, count downloads
  and uploads, and revoke them.
- Passwords are argon2id with their own brake (5 tries / 10 min per link);
  unlocking sets an HttpOnly cookie scoped to that link for 12 hours.
- A link reaches only its own file or folder (same path rules as the app:
  no `..`, no hidden names, no symlinks out). Visitors see names and sizes,
  never drive ids or real paths. The rest of Holodrive still needs a login.
- Links work wherever Holodrive is reachable, i.e. your tailnet. Opening
  them to the internet (Tailscale Funnel) is deliberately not done for you.

## 📱 Devices (tokens for apps and scripts)

The Android app, scripts and other computers can use their own **device
token** instead of the password: `Authorization: Bearer hd_…`. Each token can
be **read-only** and revoked on its own, and shows when and from where it was
last used. Make them on the 📲 **Devices** page or in a terminal:

```sh
holodrive token add "Pixel 9" [--read-only]   # prints the token once
holodrive token list
holodrive token revoke <id>
```

Only a SHA-256 of each token is kept (`~/.config/holodrive/tokens.json`,
mode 0600), and the server picks up changes without a restart. Bearer
requests need no CSRF token, because browsers never send that header on
their own. No token can manage Samba, tokens or share links; that needs a
browser session.

```sh
curl -H "Authorization: Bearer $TOKEN" http://blackbox.tail1234.ts.net:8768/api/roots
```

### 🔐 GhostPort tunnel status

`GET /api/integrations/ghostport/status` returns read-only connection health
(`status/v1`) and is available to a Holodrive session or device token. The
server reads GhostPort's owner-only Unix socket locally; peer addresses and
names are removed from the response. Run both daemons as the same OS user,
or set `GHOSTPORT_STATUS_SOCKET` in the Holodrive service environment for a
nonstandard socket path. The endpoint cannot change GhostPort configuration
or control its tunnels.

### 🗝️ CyberVault share storage

Create a protected link and save its URL and passphrase into CyberVault from
the web or Android app. Run `cybervault bridge` in a local terminal on the
server first; the bridge requires local master-password unlock and auto-locks
after five idle minutes. Only password-based HOLODRIVE sessions can append a
share secret; device tokens cannot. See
[`docs/integrations/cybervault.md`](docs/integrations/cybervault.md).

## ⬆️ Resumable upload API

For apps and scripts (the web page uses it for big files). Send a device
token (`Authorization: Bearer …`), or use a browser session (cookie +
`X-Holodrive-Csrf`).

| Request | Body | Reply |
|---|---|---|
| `POST /api/upload/start` | `{"dir": "shared/videos", "name": "a/b.mp4", "size": 123, "overwrite": false}` | `{"id", "offset": 0, "size", "chunk"}` |
| `PUT /api/upload/chunk?id=…&offset=N` | the bytes from `N` (any length; `chunk` is the suggested size) | `{"offset", "size"}` |
| `GET /api/upload/status?id=…` | | `{"offset", "size"}` |
| `POST /api/upload/finish` | `{"id"}` | `{"path", "size"}` (final name never clobbers unless `overwrite`) |
| `POST /api/upload/cancel` | `{"id"}` | `{"ok": true}` |

A chunk whose `offset` isn't what the server has gets `409` with the right
`offset`; a chunk past the announced size gets `413`. Bytes that arrive
before a connection drops are kept, so after an error ask `status` and
continue. `start` refuses files bigger than the drive's free space (`507`).
Data waits in `<drive>/.holodrive-uploads/` (same filesystem, so `finish` is
a rename) and is removed after a day without progress.

## 🧭 Roadmap

- (open) ideas welcome.

The editor bundle (`static/editor.js.gz`) is built from `editor/` with
`editor/build.sh`; CodeMirror and its packages are MIT.

MIT © Cybercore Tech
