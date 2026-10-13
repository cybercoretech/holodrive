<p align="center"><img src="static/holocube.svg" width="96" alt=""></p>

# HOLODRIVE

**A neon web file manager for your own machine, reached over your tailnet.**
Browse, preview, upload, move and restore the folders you share — from the
desktop, another box, or your phone — in the same CYBERGRID look as the rest of
Cybercore. One Rust binary, no database, no Node.

## What it does

- **Drives.** Each shared folder is a drive in the sidebar, with free space and
  a read-only badge where it applies (e.g. ISOs).
- **Grid or list.** Glowing holographic folders tinted per name, emoji tags for
  known folders (📱 `from-phone`, 🤝 `handoffs`, 🧬 `src`…), file-type emoji,
  and real thumbnails for pictures and videos.
- **Lightbox.** Pictures with zoom (wheel, double-click, pinch), drag to pan,
  swipe or ←/→ between them, and a filmstrip. Videos stream with seeking; audio
  plays on a spinning disc; PDFs open inline.
- **Read anything textual.** Code with line numbers and highlighting, and
  Markdown rendered safely (no raw HTML).
- **Upload anywhere.** Drag files or whole folders onto the page, paste
  (Ctrl+V a screenshot), or open the upload panel (its own neon drop zone, a
  staged list and a destination picker; the browser's file chooser only opens
  if you ask for it). Parallel uploads with live progress; nothing lands until
  it's complete; names never clobber (`photo (2).jpg`).
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
- **Search** a drive by name.
- **Properties** show permissions, the real path, and the Samba path
  (`\\blackbox\Shared\…`, `smb://…`) to copy.
- **70+ themes.** The theme picker uses the shared Cybercore catalog. It's per
  device, so picking a theme on the phone doesn't repaint the desktop; until you
  pick one, it follows the desktop's active theme live.
- **Keyboard.** Arrows, Enter, Space (preview), Backspace (up), Delete, F2,
  Ctrl+A/C/X/V, `/` search, `g`/`l` grid/list, `u` upload, `n` new folder,
  `e` edit, `i` properties, `r` refresh.

## Security model

- Listens on **127.0.0.1** only. `tailscale serve` publishes it to your tailnet
  (WireGuard-encrypted); nothing is opened to the LAN or the internet.
- **One password**, stored as an argon2id hash. A login sets an HttpOnly,
  SameSite=Strict session cookie; every change also needs the session's CSRF
  token and an allowed Origin. Only known `Host` names are accepted (DNS
  rebinding). Five wrong passwords in ten minutes pause logins.
- **Paths can't escape.** Every path is resolved against its drive; `..`,
  hidden names (dot files: `.git`, `.env`, `.snapshots`) and symlinks that lead
  outside the drive are refused. Read-only drives refuse every change.
- **Files can't attack the page.** Raw files are served with
  `Content-Security-Policy: sandbox`; HTML and SVG download instead of opening.
  The app itself runs under a strict CSP with no inline script.

## Install

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

## Samba helper (optional)

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
- every new smb.conf passes `testparm` before it replaces the old one, which
  is backed up to `/var/lib/holodrive/smb-backups` (last 20 kept);
- every change is logged: `journalctl -t holodrive-sambactl`.

## Roadmap

- **Time machine:** browse and restore from snapper snapshots.

The editor bundle (`static/editor.js.gz`) is built from `editor/` with
`editor/build.sh`; CodeMirror and its packages are MIT.

MIT © Cybercore Tech
