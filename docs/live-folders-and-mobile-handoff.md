# Live folders and mobile handoff

Holodrive keeps an open browser folder fresh when files are created or changed
outside the web UI. The authenticated `GET /api/events?path=<virtual-path>`
endpoint streams a `folder-change` event when the folder listing differs from
its previous snapshot. The server checks the path through the normal drive
resolver and stops the stream during graceful shutdown. It checks every two
seconds so Samba, LocalSend, and other local writers are visible too.

The browser subscribes only while browsing a folder. On an event it refreshes
that listing while retaining selected items and the current scroll position.
Android refreshes the visible Files or Photos folder every 15 seconds while
the app is resumed; background work remains governed by WorkManager and
Android's network and battery policies.

The web and Android **Recent activity** views read the same authenticated
`GET /api/activity` feed. It holds up to 100 summaries in memory: folder changes
observed by active watchers, completed server uploads, and Android sync runs
(including queued uploads, downloads, and conflict counts). It contains paths
and short summaries, not file contents, and clears on server restart. It is a
shared recent feed rather than a durable operation log.

The Photos endpoint reads the standard JPEG EXIF capture timestamp from the
file header when available. It returns that value as camera-local text (EXIF
usually has no timezone), and the web and Android galleries use it for year
grouping or ordering. Images without that tag fall back to modified time.

The web **Send to phone** action shows a QR code and copies a
`holodrive://open?path=...` deep link. QR generation uses the optional local
`qrencode` command; if it is unavailable, the dialog still exposes the copied
link.
Holodrive Photos accepts only that custom scheme and an app-local virtual path,
then opens it in Files after the user signs in. The link does not contain the
server address, password, cookie, or device token. The app uses the already
configured server and the server still enforces its normal authentication,
Host allowlist, path checks, and read-only rules.

## Two-way sync conflict policy

In two-way mode, a copy with a clearly newer modified time wins. When the
timestamps are within the filesystem tolerance but file sizes differ, the
worker copies neither version and reports the relative path in Sync settings.
This is a conservative signal for review; equal-size files with conflicting
contents cannot be identified by this metadata-only comparison. One-way modes
retain their explicit source-of-truth behavior. Sync never propagates
deletions.
