"use strict";
// HOLODRIVE — single-page client. Talks to the JSON API; builds DOM with
// h() (never innerHTML with file data). Hash routes:
//   #/browse/<root>/<path…>   #/search/<root>/<path…>?q=…   #/trash

// ── tiny helpers ───────────────────────────────────────────────────────
const $ = (sel, el = document) => el.querySelector(sel);
const $$ = (sel, el = document) => [...el.querySelectorAll(sel)];
function h(tag, attrs = {}, ...kids) {
  const el = document.createElementNS(tag === "svg" || tag === "path" || tag === "defs" || tag === "linearGradient" || tag === "stop" ? "http://www.w3.org/2000/svg" : "http://www.w3.org/1999/xhtml", tag);
  for (const [k, v] of Object.entries(attrs || {})) {
    if (v == null || v === false) continue;
    if (k === "text") el.textContent = v;
    else if (k === "class") el.setAttribute("class", v);
    else if (k === "style" && typeof v === "object") { for (const [sk, sv] of Object.entries(v)) sk.startsWith("--") ? el.style.setProperty(sk, sv) : (el.style[sk] = sv); }
    else if (k.startsWith("on") && typeof v === "function") el.addEventListener(k.slice(2), v);
    else if (k === "dataset") { for (const [dk, dv] of Object.entries(v)) if (dv != null) el.dataset[dk] = dv; }
    else el.setAttribute(k, v === true ? "" : v);
  }
  for (const kid of kids.flat()) if (kid != null && kid !== false) el.append(kid.nodeType ? kid : String(kid));
  return el;
}
const enc = encodeURIComponent;
const encPath = (p) => p.split("/").map(enc).join("/");
const decPath = (p) => p.split("/").filter(Boolean).map(decodeURIComponent).join("/");
const store = {
  get(k, d) { try { const v = localStorage.getItem("holodrive." + k); return v == null ? d : JSON.parse(v); } catch { return d; } },
  set(k, v) { try { localStorage.setItem("holodrive." + k, JSON.stringify(v)); } catch { /* private mode */ } },
};
const isTouch = matchMedia("(pointer: coarse)").matches;
const reduceMotion = matchMedia("(prefers-reduced-motion: reduce)").matches;

function fmtSize(n) {
  if (n == null) return "";
  const u = ["B", "KB", "MB", "GB", "TB"];
  let i = 0;
  while (n >= 1024 && i < u.length - 1) { n /= 1024; i++; }
  return (i ? n.toFixed(n < 10 ? 1 : 0) : n) + " " + u[i];
}
function fmtDate(secs) {
  if (!secs) return "";
  const d = new Date(secs * 1000), now = Date.now(), diff = (now - d) / 1000;
  if (diff < 60) return "just now";
  if (diff < 3600) return Math.floor(diff / 60) + " min ago";
  if (diff < 86400 && d.getDate() === new Date().getDate()) return "today " + d.toTimeString().slice(0, 5);
  if (diff < 7 * 86400) return d.toLocaleDateString(undefined, { weekday: "short" }) + " " + d.toTimeString().slice(0, 5);
  return d.toISOString().slice(0, 10) + " " + d.toTimeString().slice(0, 5);
}
function fmtMode(m) {
  const r = (b, c) => (m & b ? c : "-");
  return [r(256, "r"), r(128, "w"), r(64, "x"), r(32, "r"), r(16, "w"), r(8, "x"), r(4, "r"), r(2, "w"), r(1, "x")].join("") + "  (" + (m & 0o777).toString(8) + ")";
}
const parentOf = (p) => p.split("/").slice(0, -1).join("/");
const baseName = (p) => p.split("/").pop();

// ── API ────────────────────────────────────────────────────────────────
let CSRF = null;
async function api(method, url, body) {
  const opts = { method, headers: {}, credentials: "same-origin" };
  if (method !== "GET") opts.headers["X-Holodrive-Csrf"] = CSRF || "";
  if (body !== undefined) { opts.headers["Content-Type"] = "application/json"; opts.body = JSON.stringify(body); }
  const r = await fetch(url, opts);
  if (r.status === 401 && !url.startsWith("/api/login")) { showLogin(); throw new Error("signed out"); }
  const ct = r.headers.get("content-type") || "";
  const data = ct.includes("json") ? await r.json() : await r.text();
  if (!r.ok) throw new Error((data && data.error) || data || r.statusText);
  return data;
}

// ── file kinds & icons ─────────────────────────────────────────────────
const EXT = {
  image: "jpg jpeg png gif webp bmp svg avif ico tif tiff",
  video: "mp4 webm mkv mov m4v ogv avi",
  audio: "mp3 flac ogg oga wav m4a opus aac",
  pdf: "pdf",
  md: "md markdown",
  text: "txt log csv tsv json jsonc json5 toml yaml yml ini conf cfg env rs py js mjs cjs ts tsx jsx go rb lua sh bash zsh fish c h cpp hpp cc java kt swift php pl r sql html htm css scss sass less xml svgz vue svelte nix diff patch lock gitignore dockerfile makefile cmake gradle properties service timer socket desktop rules policy srt vtt tex bib org rst adoc el vim kdl ron zig dart ex exs erl hs ml clj scala cs fs ps1 bat awk sed",
};
const KIND = {};
for (const [k, list] of Object.entries(EXT)) for (const e of list.split(" ")) KIND[e] = k;
const TEXT_NAMES = new Set(["makefile", "dockerfile", "license", "readme", "changelog", "pkgbuild", "justfile", "procfile", "cargo.lock", "copying", "authors", "todo"]);
function kindOf(entry) {
  if (entry.kind === "dir") return "dir";
  const k = KIND[entry.ext];
  if (k) return k;
  if (TEXT_NAMES.has(entry.name.toLowerCase())) return "text";
  if ((entry.mime || "").startsWith("text/")) return "text";
  return "other";
}
const ICON = {
  rs: "🦀", py: "🐍", go: "🐹", rb: "💎", js: "📜", mjs: "📜", ts: "📘", tsx: "📘", jsx: "⚛️", lua: "🌙", sh: "🐚", bash: "🐚", zsh: "🐚", fish: "🐟",
  c: "⚙️", h: "⚙️", cpp: "⚙️", hpp: "⚙️", java: "☕", kt: "🟪", swift: "🕊️", php: "🐘", html: "🌐", htm: "🌐", css: "🎨", scss: "🎨",
  json: "🧾", toml: "🧾", yaml: "🧾", yml: "🧾", ini: "🧾", conf: "🧾", cfg: "🧾", xml: "🧾", lock: "🔒", env: "🔑", sql: "🗄️",
  md: "📝", markdown: "📝", txt: "📄", log: "📜", csv: "📊", tsv: "📊", xlsx: "📊", xls: "📊", ods: "📊",
  pdf: "📕", doc: "📘", docx: "📘", odt: "📘", ppt: "📽️", pptx: "📽️", odp: "📽️", epub: "📖",
  zip: "📦", tar: "📦", gz: "📦", tgz: "📦", xz: "📦", zst: "📦", "7z": "📦", rar: "📦", bz2: "📦", deb: "📦", rpm: "📦",
  iso: "💿", img: "💿", qcow2: "💿", vdi: "💿", appimage: "🚀", exe: "🚀", msi: "🚀", apk: "🤖", jar: "☕",
  ttf: "🔤", otf: "🔤", woff: "🔤", woff2: "🔤", svg: "✒️", psd: "🎨", kra: "🎨", xcf: "🎨", blend: "🧊", obj: "🧊", stl: "🧊", gltf: "🧊", glb: "🧊",
  db: "🗄️", sqlite: "🗄️", sqlite3: "🗄️", key: "🔐", pem: "🔐", crt: "🔐", gpg: "🔐", asc: "🔐", torrent: "🧲", ics: "📅", vcf: "📇",
  service: "🛠️", timer: "⏱️", socket: "🔌", desktop: "🖥️", diff: "🩹", patch: "🩹", nix: "❄️", zig: "⚡", dart: "🎯", ex: "💧", exs: "💧",
  html5: "🌐", srt: "💬", vtt: "💬", wasm: "🧩",
};
const KIND_ICON = { image: "🖼️", video: "🎬", audio: "🎵", pdf: "📕", md: "📝", text: "📄", other: "📄", dir: "📁" };
const iconFor = (e) => (e.kind === "dir" ? "📁" : ICON[e.ext] || KIND_ICON[kindOf(e)] || "📄");
const FOLDER_EMOJI = {
  "from-phone": "📱", "to-darkbox": "🛰️", "to-blackbox": "📡", handoffs: "🤝", common: "🧩", scratch: "🧪", src: "🧬", source: "🧬",
  docs: "📚", doc: "📚", documentation: "📚", assets: "🎨", images: "🖼️", img: "🖼️", pictures: "🖼️", photos: "📸", screenshots: "📸",
  wallpapers: "🌄", music: "🎵", audio: "🎧", videos: "🎬", video: "🎬", movies: "🍿", downloads: "📥", target: "⚙️", build: "🏗️",
  dist: "📦", node_modules: "📦", vendor: "📦", packaging: "📦", tests: "🧪", test: "🧪", scripts: "📜", bin: "⚙️", config: "🛠️",
  static: "🧱", public: "🌍", templates: "📐", backups: "💾", backup: "💾", archive: "🗄️", archives: "🗄️", fonts: "🔤", games: "🎮",
  notes: "🗒️", wiki: "🧠", web: "🌐", site: "🌐", docker: "🐳", containers: "🐳", security: "🛡️", network: "🕸️", core: "🧿",
  tooling: "🧰", integrations: "🔗", framework: "🏛️", aur: "📦", themes: "🎨", examples: "🧪", demo: "🎪", lib: "📚", migrations: "🧬",
  cybercore: "🧿", private: "🔒", secrets: "🔒", keys: "🔑", ai: "🤖", agents: "🤖", models: "🧠", iso: "💿", isos: "💿",
};
const FOLDER_COLORS = ["--cyan", "--pink", "--purple", "--acid", "--orange"];
function folderColor(name) {
  let x = 0;
  for (const c of name) x = (x * 31 + c.charCodeAt(0)) >>> 0;
  return `var(${FOLDER_COLORS[x % FOLDER_COLORS.length]})`;
}
function folderArt(name) {
  const svg = h("svg", { viewBox: "0 0 84 68", "aria-hidden": "true" },
    h("path", { class: "f-back", d: "M4 10a6 6 0 0 1 6-6h18l7 7h39a6 6 0 0 1 6 6v41a6 6 0 0 1-6 6H10a6 6 0 0 1-6-6Z" }),
    h("path", { class: "f-front", d: "M4 24a6 6 0 0 1 6-6h64a6 6 0 0 1 6 6v34a6 6 0 0 1-6 6H10a6 6 0 0 1-6-6Z" }),
    h("path", { class: "f-shine", d: "M6 24a5 5 0 0 1 5-5h62a5 5 0 0 1 5 5v9H6Z" }),
    h("path", { class: "f-line", d: "M10 56h64", fill: "none" }));
  const emoji = FOLDER_EMOJI[name.toLowerCase()];
  return h("div", { class: "folder", style: { "--fc": folderColor(name) } }, svg, emoji ? h("span", { class: "f-emoji emoji", text: emoji }) : null);
}
const hasThumb = (e) => e.kind === "file" && (["jpg", "jpeg", "png", "gif", "webp", "bmp", "tif", "tiff"].includes(e.ext) || ["mp4", "mkv", "webm", "mov", "avi", "m4v"].includes(e.ext));
const thumbUrl = (path) => `/api/thumb?path=${enc(path)}`;
const rawUrl = (path, dl) => `/api/raw?path=${enc(path)}${dl ? "&dl=1" : ""}`;

// ── state ──────────────────────────────────────────────────────────────
const S = {
  session: null, roots: [], path: "", listing: null, entries: [], sel: new Set(), focus: -1, anchor: -1,
  view: store.get("view", isTouch ? "grid" : "grid"), sort: store.get("sort", "name"), dir: store.get("dir", 1),
  clip: null, route: "browse", search: null,
};
const entryPath = (e) => e.path || (S.path ? S.path + "/" + e.name : e.name);

// ── toasts ─────────────────────────────────────────────────────────────
function toast(msg, kind = "", action) {
  const el = h("div", { class: "toast " + kind }, h("span", { text: msg }));
  if (action) el.append(h("button", { class: "btn", type: "button", text: action.label, onclick: () => { action.run(); el.remove(); } }));
  $("#toasts").append(el);
  setTimeout(() => el.remove(), action ? 9000 : kind === "err" ? 7000 : 3500);
}
const fail = (e) => toast(e.message || String(e), "err");

// ── boot & login ───────────────────────────────────────────────────────
async function bootLog() {
  const lines = ["> init holo-lattice ......... ok", "> mount drives ............. ok", "> tailnet uplink ............ secured", "> render neon ............... ✓"];
  const log = $("#boot-log");
  if (reduceMotion) { log.textContent = lines.join("\n"); return; }
  for (const l of lines) { log.textContent += l + "\n"; await new Promise((r) => setTimeout(r, 120)); }
}

function showLogin() {
  $("#app").hidden = true;
  $("#login").hidden = false;
  document.body.classList.remove("booting");
  setTimeout(() => $("#login-pass").focus(), 50);
}

async function start() {
  const [, sess] = await Promise.all([bootLog(), api("GET", "/api/session").catch(() => null)]);
  S.session = sess;
  if (sess) {
    $("#login-host").textContent = `${sess.host} · ${sess.user}`;
    $("#host-name").textContent = sess.host;
    document.title = `HOLODRIVE · ${sess.host}`;
  }
  setupTheme().catch(() => {});
  if (sess && sess.authed) { CSRF = sess.csrf; await enterApp(); }
  else {
    showLogin();
    if (sess && !sess.configured) $("#login-msg").textContent = "No password set yet — run `holodrive passwd` on the host.";
  }
}

$("#login-form").addEventListener("submit", async (ev) => {
  ev.preventDefault();
  const btn = $("#login-btn"), msg = $("#login-msg");
  btn.disabled = true; msg.textContent = "";
  try {
    const r = await api("POST", "/api/login", { password: $("#login-pass").value });
    CSRF = r.csrf;
    $("#login-pass").value = "";
    $("#login").hidden = true;
    await enterApp();
  } catch (e) {
    msg.textContent = e.message;
    const w = $(".login-window"); w.classList.remove("shake"); void w.offsetWidth; w.classList.add("shake");
  } finally { btn.disabled = false; }
});

$("#logout-btn").addEventListener("click", async () => {
  await api("POST", "/api/logout").catch(() => {});
  CSRF = null; showLogin();
});

let appWired = false;
async function enterApp() {
  $("#app").hidden = false;
  document.body.classList.remove("booting");
  if (!appWired) { wireApp(); appWired = true; }
  await loadRoots();
  refreshTrashCount();
  await route();
}

// ── drives sidebar ─────────────────────────────────────────────────────
async function loadRoots() {
  S.roots = await api("GET", "/api/roots");
  const nav = $("#drives");
  nav.replaceChildren(...S.roots.map((r) => {
    const used = r.total ? 1 - r.free / r.total : 0;
    return h("a", {
      class: "drive", href: "#/browse/" + enc(r.id), dataset: { root: r.id, drop: r.id },
      title: r.online ? `${fmtSize(r.free)} free of ${fmtSize(r.total)}` : "offline",
    },
      h("span", { class: "drive-icon emoji", text: r.icon }),
      h("span", { class: "drive-name", text: r.name }),
      r.writable ? h("span") : h("span", { class: "drive-ro", text: "RO" }),
      h("span", { class: "drive-bar" }, h("i", { style: { width: (used * 100).toFixed(1) + "%" } })));
  }));
}
async function refreshTrashCount() {
  try {
    const t = await api("GET", "/api/trash");
    $("#trash-count").textContent = t.items.length ? String(t.items.length) : "";
  } catch { /* ignore */ }
}
function markNav() {
  const root = S.path.split("/")[0];
  $$(".drive").forEach((d) => d.classList.toggle("active",
    (S.route === "trash" && d.dataset.nav === "trash") || (S.route !== "trash" && d.dataset.root === root)));
}

// ── routing ────────────────────────────────────────────────────────────
async function route() {
  closeViewer(); closeCtx(); $("#sidebar").classList.remove("open");
  const hash = location.hash.slice(1) || "/";
  const [pathPart, query] = hash.split("?");
  const parts = pathPart.split("/").filter(Boolean);
  if (parts[0] === "trash") return showTrash();
  if (parts[0] === "search") {
    const q = new URLSearchParams(query || "").get("q") || "";
    return showSearch(decPath(parts.slice(1).join("/")), q);
  }
  if (parts[0] === "browse" && parts.length > 1) return browse(decPath(parts.slice(1).join("/")));
  const last = store.get("last", null);
  const first = S.roots.find((r) => r.online);
  location.replace("#/browse/" + encPath(last && S.roots.some((r) => r.id === last.split("/")[0]) ? last : first ? first.id : ""));
}
window.addEventListener("hashchange", () => { if (CSRF) route(); });
const go = (vpath) => { location.hash = "#/browse/" + encPath(vpath); };

// ── browse ─────────────────────────────────────────────────────────────
async function browse(vpath, keepSel) {
  S.route = "browse"; S.path = vpath; S.search = null;
  $("#search-input").value = "";
  const view = $("#view");
  if (!S.listing || S.listing.path !== vpath) view.replaceChildren(h("div", { class: "loading", text: "decrypting sector…" }));
  try {
    S.listing = await api("GET", "/api/list?path=" + enc(vpath));
  } catch (e) {
    view.replaceChildren(h("div", { class: "empty" }, h("div", { class: "empty-icon", text: "⚠️" }), h("div", { text: e.message })));
    renderCrumbs(); markNav(); return;
  }
  store.set("last", vpath);
  if (!keepSel) { S.sel.clear(); S.focus = -1; S.anchor = -1; }
  S.entries = sortEntries(S.listing.entries);
  render();
}
async function refresh() {
  if (S.route === "browse") await browse(S.path, true);
  else if (S.route === "trash") await showTrash();
  else if (S.route === "search") await showSearch(S.path, S.search);
  loadRoots().catch(() => {});
  refreshTrashCount();
}

function sortEntries(list) {
  const key = {
    name: (e) => e.name.toLowerCase(),
    mtime: (e) => e.mtime,
    size: (e) => e.size,
    type: (e) => (e.kind === "dir" ? "" : e.ext + "\0" + e.name.toLowerCase()),
  }[S.sort];
  return [...list].sort((a, b) => {
    if ((a.kind === "dir") !== (b.kind === "dir")) return a.kind === "dir" ? -1 : 1;
    const ka = key(a), kb = key(b);
    return (ka < kb ? -1 : ka > kb ? 1 : 0) * S.dir;
  });
}

function renderCrumbs() {
  const crumbs = $("#crumbs");
  if (S.route === "trash") { crumbs.replaceChildren(h("span", { class: "crumb last", text: "🗑️ Trash" })); return; }
  const parts = S.path.split("/").filter(Boolean);
  const root = S.roots.find((r) => r.id === parts[0]);
  const out = [];
  parts.forEach((p, i) => {
    const vp = parts.slice(0, i + 1).join("/");
    const label = i === 0 ? `${root ? root.icon : "📁"} ${root ? root.name : p}` : p;
    if (i) out.push(h("span", { class: "crumb-sep", text: "/" }));
    out.push(h("a", { class: "crumb" + (i === parts.length - 1 && S.route === "browse" ? " last" : ""), href: "#/browse/" + encPath(vp), dataset: { drop: vp }, text: label }));
  });
  if (S.route === "search") out.push(h("span", { class: "crumb-sep", text: "⌕" }), h("span", { class: "crumb last", text: `“${S.search}”` }));
  crumbs.replaceChildren(...out);
}

function render() {
  renderCrumbs(); markNav();
  const writable = S.route === "browse" && S.listing && S.listing.writable;
  for (const id of ["upload-btn", "mkdir-btn", "newfile-btn", "fab"]) $("#" + id).hidden = !writable;
  $("#toolbar").hidden = S.route === "trash";
  $$(".seg-btn").forEach((b) => b.classList.toggle("on", b.dataset.view === S.view));
  $("#sort-select").value = S.sort;
  $("#sort-dir").textContent = S.dir > 0 ? "↑" : "↓";
  const view = $("#view");
  if (!S.entries.length) {
    view.replaceChildren(h("div", { class: "empty" },
      h("div", { class: "empty-icon emoji", text: S.route === "search" ? "🔭" : "🌌" }),
      h("div", { text: S.route === "search" ? "No matches in this sector." : "Empty sector." }),
      writable ? h("div", { class: "muted", text: "Drop files here or press U to upload." }) : null));
    updateSelbar(); return;
  }
  view.replaceChildren(S.view === "list" ? renderList() : renderGrid());
  updateSelbar();
}

function tileArt(e, path) {
  if (e.kind === "dir") return folderArt(e.name);
  if (hasThumb(e)) {
    const img = h("img", { class: "thumb", loading: "lazy", decoding: "async", alt: "", src: thumbUrl(path), draggable: "false" });
    img.addEventListener("error", () => img.replaceWith(h("span", { class: "big-emoji emoji", text: iconFor(e) })), { once: true });
    const wrap = [img];
    if (kindOf(e) === "video") wrap.push(h("span", { class: "play", text: "▶" }));
    return wrap;
  }
  return h("span", { class: "big-emoji emoji", text: iconFor(e) });
}

function renderGrid() {
  const grid = h("div", { class: "grid" + (S.sel.size ? " selecting" : "") });
  S.entries.forEach((e, i) => {
    const path = entryPath(e);
    grid.append(h("div", {
      class: "tile" + (S.sel.has(path) ? " sel" : "") + (i === S.focus ? " focus" : "") + (S.clip && S.clip.mode === "cut" && S.clip.paths.includes(path) ? " cut" : ""),
      dataset: { i, drop: e.kind === "dir" ? path : null }, draggable: S.route === "browse" && !isTouch ? "true" : null,
      title: `${e.name}\n${e.kind === "dir" ? "folder" : fmtSize(e.size)} · ${fmtDate(e.mtime)}`,
    },
      h("span", { class: "tile-check", "data-check": "1", text: S.sel.has(path) ? "✓" : "" }),
      e.link ? h("span", { class: "tile-badge", title: e.escapes ? "link outside this drive (blocked)" : "symlink", text: e.escapes ? "🚫" : "🔗" }) : null,
      h("div", { class: "tile-art" }, tileArt(e, path)),
      h("div", { class: "tile-name", text: e.name }),
      S.route === "search" ? h("div", { class: "tile-sub", text: parentOf(path) }) : h("div", { class: "tile-sub", text: e.kind === "dir" ? fmtDate(e.mtime) : fmtSize(e.size) })));
  });
  return grid;
}

function renderList() {
  const list = h("div", { class: "list" + (S.sel.size ? " selecting" : "") });
  const arrow = (k) => (S.sort === k ? (S.dir > 0 ? " ↑" : " ↓") : "");
  list.append(h("div", { class: "row head" }, h("span"), h("span"),
    h("span", { dataset: { sort: "name" }, text: "name" + arrow("name") }),
    h("span", { class: "row-size", dataset: { sort: "size" }, text: "size" + arrow("size") }),
    h("span", { class: "row-date", dataset: { sort: "mtime" }, text: "modified" + arrow("mtime") })));
  S.entries.forEach((e, i) => {
    const path = entryPath(e);
    const icon = hasThumb(e) && kindOf(e) === "image"
      ? h("img", { src: thumbUrl(path), loading: "lazy", alt: "", draggable: "false" })
      : e.kind === "dir" ? h("span", { class: "emoji", text: FOLDER_EMOJI[e.name.toLowerCase()] || "📁" }) : h("span", { class: "emoji", text: iconFor(e) });
    list.append(h("div", {
      class: "row" + (S.sel.has(path) ? " sel" : "") + (i === S.focus ? " focus" : "") + (S.clip && S.clip.mode === "cut" && S.clip.paths.includes(path) ? " cut" : ""),
      dataset: { i, drop: e.kind === "dir" ? path : null }, draggable: S.route === "browse" && !isTouch ? "true" : null,
    },
      h("span", { class: "tile-check", "data-check": "1", text: S.sel.has(path) ? "✓" : "" }),
      h("span", { class: "row-icon" }, icon),
      h("span", { class: "row-name", title: path, text: S.route === "search" ? path : e.name + (e.link ? "  🔗" : "") }),
      h("span", { class: "row-size", text: e.kind === "dir" ? "—" : fmtSize(e.size) }),
      h("span", { class: "row-date", text: fmtDate(e.mtime) })));
  });
  return list;
}

// ── selection ──────────────────────────────────────────────────────────
function selected() { return S.entries.filter((e) => S.sel.has(entryPath(e))); }
function setSel(paths) { S.sel = new Set(paths); paintSel(); }
function paintSel() {
  const items = $$("#view [data-i]");
  for (const el of items) {
    const e = S.entries[+el.dataset.i];
    const on = S.sel.has(entryPath(e));
    el.classList.toggle("sel", on);
    el.classList.toggle("focus", +el.dataset.i === S.focus);
    const c = el.querySelector(".tile-check"); if (c) c.textContent = on ? "✓" : "";
  }
  const box = $("#view .grid, #view .list"); if (box) box.classList.toggle("selecting", S.sel.size > 0);
  updateSelbar();
}
function updateSelbar() {
  const n = S.sel.size;
  $("#selbar").hidden = n === 0 || S.route === "trash";
  if (!n) return;
  const sel = selected();
  const bytes = sel.reduce((a, e) => a + (e.kind === "file" ? e.size : 0), 0);
  $("#sel-count").textContent = `${n} selected${bytes ? " · " + fmtSize(bytes) : ""}`;
  const ro = !(S.listing && S.listing.writable) && S.route === "browse";
  $$("#selbar [data-act]").forEach((b) => {
    const a = b.dataset.act;
    b.hidden = (a === "rename" && n !== 1) || ((a === "move" || a === "trash" || a === "rename") && ro);
  });
}
function clickItem(i, ev) {
  const e = S.entries[i], path = entryPath(e);
  if (ev.shiftKey && S.anchor >= 0) {
    const [a, b] = [Math.min(S.anchor, i), Math.max(S.anchor, i)];
    const add = S.entries.slice(a, b + 1).map(entryPath);
    S.sel = new Set(ev.ctrlKey || ev.metaKey ? [...S.sel, ...add] : add);
  } else if (ev.ctrlKey || ev.metaKey || ev.toggle) {
    S.sel.has(path) ? S.sel.delete(path) : S.sel.add(path);
    S.anchor = i;
  } else {
    S.sel = new Set([path]); S.anchor = i;
  }
  S.focus = i; paintSel();
}

// ── open & viewer ──────────────────────────────────────────────────────
function openEntry(e) {
  const path = entryPath(e);
  if (e.kind === "dir") return go(path);
  const files = S.entries.filter((x) => x.kind === "file");
  openViewer(files, files.findIndex((x) => entryPath(x) === path));
}

const V = { list: [], i: 0, zoom: 1, tx: 0, ty: 0 };
function openViewer(list, i) {
  V.list = list; V.i = Math.max(0, i);
  $("#viewer").hidden = false;
  document.body.style.overflow = "hidden";
  showViewerItem();
}
function closeViewer() {
  if ($("#viewer").hidden) return;
  $("#viewer").hidden = true;
  document.body.style.overflow = "";
  $("#viewer-stage").replaceChildren();
}
function viewerStep(d) {
  if (V.list.length < 2) return;
  V.i = (V.i + d + V.list.length) % V.list.length;
  showViewerItem();
}
async function showViewerItem() {
  const e = V.list[V.i]; if (!e) return;
  const path = entryPath(e), kind = kindOf(e), stage = $("#viewer-stage");
  V.zoom = 1; V.tx = 0; V.ty = 0;
  $("#viewer-icon").textContent = iconFor(e);
  $("#viewer-name").textContent = e.name;
  $("#viewer-meta").textContent = `${fmtSize(e.size)} · ${fmtDate(e.mtime)} · ${V.i + 1}/${V.list.length}`;
  $("#viewer-dl").href = rawUrl(path, true);
  $("#viewer-open").href = rawUrl(path);
  $("#viewer-prev").hidden = $("#viewer-next").hidden = V.list.length < 2;
  const media = V.list.filter((x) => ["image", "video"].includes(kindOf(x)));
  const strip = $("#viewer-strip");
  if (["image", "video"].includes(kind) && media.length > 1) {
    strip.hidden = false;
    strip.replaceChildren(...media.map((m) => {
      const img = h("img", { src: hasThumb(m) ? thumbUrl(entryPath(m)) : "", loading: "lazy", alt: m.name, title: m.name, class: m === e ? "on" : "" });
      img.addEventListener("click", () => { V.i = V.list.indexOf(m); showViewerItem(); });
      return img;
    }));
    const on = strip.querySelector(".on"); if (on) on.scrollIntoView({ inline: "center", block: "nearest", behavior: reduceMotion ? "auto" : "smooth" });
  } else strip.hidden = true;

  if (kind === "image") {
    const img = h("img", { class: "full", src: rawUrl(path), alt: e.name, draggable: "false" });
    img.addEventListener("load", () => { $("#viewer-meta").textContent = `${img.naturalWidth}×${img.naturalHeight} · ${fmtSize(e.size)} · ${V.i + 1}/${V.list.length}`; });
    stage.replaceChildren(img); wireZoom(img);
  } else if (kind === "video") {
    stage.replaceChildren(h("video", { src: rawUrl(path), controls: true, autoplay: true, playsinline: true, preload: "metadata" }));
  } else if (kind === "audio") {
    const disc = h("div", { class: "audio-disc emoji", text: "🎵" });
    const audio = h("audio", { src: rawUrl(path), controls: true, autoplay: true });
    audio.addEventListener("play", () => disc.classList.add("playing"));
    audio.addEventListener("pause", () => disc.classList.remove("playing"));
    stage.replaceChildren(h("div", { class: "audio-card" }, disc, h("div", { text: e.name }), audio));
  } else if (kind === "pdf") {
    stage.replaceChildren(h("iframe", { src: rawUrl(path), title: e.name }));
  } else if (kind === "md" || kind === "text") {
    stage.replaceChildren(h("div", { class: "loading", text: "decoding…" }));
    try {
      const t = await api("GET", "/api/text?path=" + enc(path));
      if (V.list[V.i] !== e) return;
      stage.replaceChildren(kind === "md" ? renderMarkdown(t.text, parentOf(path)) : renderCode(t.text, e.ext));
    } catch (err) { stage.replaceChildren(noPreview(e, path, err.message)); }
  } else stage.replaceChildren(noPreview(e, path));
}
function noPreview(e, path, why) {
  return h("div", { class: "nopreview" },
    h("div", { class: "big-emoji emoji", text: iconFor(e) }),
    h("div", { text: why || "No preview for this kind of file." }),
    h("a", { class: "btn btn-primary", href: rawUrl(path, true), text: "⬇ Download " + fmtSize(e.size) }));
}
function wireZoom(img) {
  const apply = () => { img.style.transform = `translate(${V.tx}px, ${V.ty}px) scale(${V.zoom})`; img.classList.toggle("zoomed", V.zoom > 1); };
  img.addEventListener("wheel", (ev) => { ev.preventDefault(); V.zoom = Math.min(8, Math.max(1, V.zoom * (ev.deltaY < 0 ? 1.15 : 1 / 1.15))); if (V.zoom === 1) { V.tx = V.ty = 0; } apply(); }, { passive: false });
  img.addEventListener("dblclick", () => { V.zoom = V.zoom > 1 ? 1 : 2.5; V.tx = V.ty = 0; apply(); });
  const pts = new Map(); let start = null, pinch = null;
  img.addEventListener("pointerdown", (ev) => { img.setPointerCapture(ev.pointerId); pts.set(ev.pointerId, { x: ev.clientX, y: ev.clientY }); start = { x: ev.clientX, y: ev.clientY, tx: V.tx, ty: V.ty, t: Date.now() }; if (pts.size === 2) { const [a, b] = [...pts.values()]; pinch = { d: Math.hypot(a.x - b.x, a.y - b.y), z: V.zoom }; } });
  img.addEventListener("pointermove", (ev) => {
    if (!pts.has(ev.pointerId)) return;
    pts.set(ev.pointerId, { x: ev.clientX, y: ev.clientY });
    if (pts.size === 2 && pinch) { const [a, b] = [...pts.values()]; V.zoom = Math.min(8, Math.max(1, pinch.z * Math.hypot(a.x - b.x, a.y - b.y) / pinch.d)); apply(); }
    else if (V.zoom > 1 && start) { V.tx = start.tx + ev.clientX - start.x; V.ty = start.ty + ev.clientY - start.y; apply(); }
  });
  img.addEventListener("pointerup", (ev) => {
    pts.delete(ev.pointerId);
    if (pts.size < 2) pinch = null;
    if (start && V.zoom === 1 && pts.size === 0) { const dx = ev.clientX - start.x; if (Math.abs(dx) > 60 && Date.now() - start.t < 600) viewerStep(dx < 0 ? 1 : -1); }
    if (pts.size === 0) start = null;
  });
}

// tiny syntax highlighter: strings, comments, numbers, keywords, calls
const KW = new Set("fn let mut pub use mod struct enum impl trait match if else for while loop return break continue async await move const static type where self Self super crate def class import from as with try except finally raise lambda yield pass in is not and or None True False function var new this typeof instanceof export default extends interface package func go defer chan select case switch local then end elseif do nil true false null undefined void int float bool char string echo fi esac done".split(" "));
const TOKEN = /(\/\/.*|#(?![!{[]).*|--\s.*|\/\*.*?\*\/)|("(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*'|`(?:[^`\\]|\\.)*`)|(\b\d[\d_.]*(?:e[+-]?\d+)?[a-z]*\b|\b0x[\da-f]+\b)|([A-Za-z_][\w]*)(?=\s*\()|([A-Za-z_][\w]*)/gi;
function renderCode(text, ext) {
  const pre = h("pre", { class: "code-view" });
  const lines = text.split("\n");
  const highlight = text.length < 400_000 && !["txt", "log", "csv", "tsv"].includes(ext);
  for (const line of lines.slice(0, 20000)) {
    const ln = h("span", { class: "ln" });
    if (!highlight) { ln.textContent = line; pre.append(ln); continue; }
    let last = 0;
    for (const m of line.matchAll(TOKEN)) {
      if (m.index > last) ln.append(line.slice(last, m.index));
      const [tok, c, s, n, f, w] = m;
      const cls = c ? "tok-c" : s ? "tok-s" : n ? "tok-n" : f ? "tok-f" : w && KW.has(w) ? "tok-k" : null;
      ln.append(cls ? h("span", { class: cls, text: tok }) : tok);
      last = m.index + tok.length;
    }
    if (last < line.length) ln.append(line.slice(last));
    pre.append(ln);
  }
  if (lines.length > 20000) pre.append(h("span", { class: "ln muted", text: `… ${lines.length - 20000} more lines` }));
  return pre;
}

// minimal, safe markdown: everything is built as DOM nodes
function renderMarkdown(text, baseDir) {
  const root = h("div", { class: "md-view" });
  const inline = (s) => {
    const out = [];
    const re = /(`[^`]+`)|(\*\*[^*]+\*\*)|(\*[^*\s][^*]*\*|\b_[^_]+_\b)|(\[!\[[^\]]*\]\([^)\s]+\)\]\([^)\s]+\)|!?\[[^\]]*\]\([^)\s]+\))/g;
    let last = 0;
    for (const m of s.matchAll(re)) {
      if (m.index > last) out.push(s.slice(last, m.index));
      const t = m[0];
      if (m[1]) out.push(h("code", { text: t.slice(1, -1) }));
      else if (m[2]) out.push(h("strong", {}, inline(t.slice(2, -2))));
      else if (m[3]) out.push(h("em", {}, inline(t.slice(1, -1))));
      else if (t.startsWith("[![")) {
        const label = t.slice(3, t.indexOf("]"));
        const url = t.slice(t.lastIndexOf("](") + 2, -1);
        out.push(/^https?:\/\//i.test(url) ? h("a", { href: url, target: "_blank", rel: "noopener noreferrer", class: "md-badge", text: label }) : label);
      } else {
        const img = t.startsWith("!");
        const label = t.slice(img ? 2 : 1, t.indexOf("]"));
        let url = t.slice(t.indexOf("(") + 1, -1);
        const external = /^https?:\/\//i.test(url);
        if (!external && !/^[a-z]+:/i.test(url) && !url.startsWith("#")) url = img ? rawUrl(normalize(baseDir + "/" + url)) : "#/browse/" + encPath(normalize(baseDir + "/" + url));
        if (/^(javascript|data|vbscript):/i.test(url)) out.push(label);
        else if (img) out.push(h("img", { src: external ? "" : url, alt: label, title: external ? "remote image blocked" : label }));
        else out.push(h("a", { href: url, target: external ? "_blank" : null, rel: "noopener noreferrer", text: label }));
      }
      last = m.index + t.length;
    }
    if (last < s.length) out.push(s.slice(last));
    return out;
  };
  const lines = text.split("\n");
  let i = 0, list = null;
  while (i < lines.length) {
    const l = lines[i];
    if (/^```/.test(l)) {
      const buf = []; i++;
      while (i < lines.length && !/^```/.test(lines[i])) buf.push(lines[i++]);
      root.append(h("pre", {}, h("code", { text: buf.join("\n") }))); i++; list = null; continue;
    }
    const hm = l.match(/^(#{1,6})\s+(.*)/);
    if (hm) { root.append(h("h" + hm[1].length, {}, inline(hm[2]))); list = null; i++; continue; }
    if (/^\s*([-*_])\s*\1\s*\1[\s\-*_]*$/.test(l)) { root.append(h("hr")); i++; continue; }
    const li = l.match(/^\s*(?:[-*+]|\d+\.)\s+(.*)/);
    if (li) {
      if (!list) { list = h(/^\s*\d/.test(l) ? "ol" : "ul"); root.append(list); }
      const task = li[1].match(/^\[([ xX])\]\s+(.*)/);
      list.append(h("li", {}, task ? [task[1] === " " ? "☐ " : "☑ ", ...inline(task[2])] : inline(li[1]))); i++; continue;
    }
    list = null;
    if (/^>\s?/.test(l)) { root.append(h("blockquote", {}, inline(l.replace(/^>\s?/, "")))); i++; continue; }
    if (/^\|.*\|\s*$/.test(l)) {
      const table = h("table");
      while (i < lines.length && /^\|.*\|\s*$/.test(lines[i])) {
        const cells = lines[i].trim().slice(1, -1).split("|").map((c) => c.trim());
        if (!cells.every((c) => /^:?-+:?$/.test(c))) table.append(h("tr", {}, cells.map((c) => h("td", {}, inline(c)))));
        i++;
      }
      root.append(table); continue;
    }
    if (!l.trim()) { i++; continue; }
    if (/^\s*<\/?[a-z][^>]*>\s*(<[^>]*>\s*)*$/i.test(l) || /^\s*<!--/.test(l)) { i++; continue; } // raw HTML: not rendered
    const para = [];
    while (i < lines.length && lines[i].trim() && !/^(#|```|>|\s*[-*+]\s|\s*\d+\.\s|\|)/.test(lines[i])) para.push(lines[i++]);
    if (!para.length) para.push(lines[i++]);
    root.append(h("p", {}, inline(para.join(" "))));
  }
  return root;
}
function normalize(p) {
  const out = [];
  for (const s of p.split("/")) { if (!s || s === ".") continue; if (s === "..") out.pop(); else out.push(s); }
  return out.join("/");
}

// ── dialogs ────────────────────────────────────────────────────────────
function modal(title, body, actions) {
  $("#modal-title").textContent = title;
  const acts = h("div", { class: "modal-actions" }, actions || []);
  $("#modal-body").replaceChildren(body, acts);
  $("#modal").hidden = false;
  const first = $("#modal-body input, #modal-body .btn-primary"); if (first) setTimeout(() => first.focus(), 30);
}
function closeModal() { $("#modal").hidden = true; $("#modal-body").replaceChildren(); }
function prompt(title, label, value, okText, selectStem) {
  return new Promise((resolve) => {
    const input = h("input", { class: "text-input", value: value || "", spellcheck: "false" });
    const done = (v) => { closeModal(); resolve(v); };
    const form = h("form", { class: "field", onsubmit: (ev) => { ev.preventDefault(); done(input.value.trim() || null); } },
      h("span", { class: "field-label", text: label }), input);
    modal(title, form, [h("button", { class: "btn", type: "button", text: "Cancel", onclick: () => done(null) }),
      h("button", { class: "btn btn-primary", type: "button", text: okText || "OK", onclick: () => done(input.value.trim() || null) })]);
    setTimeout(() => { input.focus(); const dot = input.value.lastIndexOf("."); input.setSelectionRange(0, selectStem && dot > 0 ? dot : input.value.length); }, 40);
  });
}
function confirmBox(title, text, okText, danger) {
  return new Promise((resolve) => {
    const done = (v) => { closeModal(); resolve(v); };
    modal(title, h("p", { text }), [h("button", { class: "btn", type: "button", text: "Cancel", onclick: () => done(false) }),
      h("button", { class: "btn " + (danger ? "btn-danger" : "btn-primary"), type: "button", text: okText, onclick: () => done(true) })]);
  });
}

async function doMkdir() {
  const name = await prompt("New folder", "folder name", "", "Create");
  if (!name) return;
  try { const r = await api("POST", "/api/mkdir", { dir: S.path, name }); await refresh(); focusPath(r.path); toast("📁 " + name + " created", "ok"); } catch (e) { fail(e); }
}
async function doNewFile() {
  const name = await prompt("New file", "file name", "notes.md", "Create", true);
  if (!name) return;
  try { const r = await api("POST", "/api/newfile", { dir: S.path, name }); await refresh(); focusPath(r.path); toast("📝 " + name + " created", "ok"); } catch (e) { fail(e); }
}
async function doRename(e) {
  const name = await prompt("Rename", "new name", e.name, "Rename", e.kind !== "dir");
  if (!name || name === e.name) return;
  try { const r = await api("POST", "/api/rename", { path: entryPath(e), name }); await refresh(); focusPath(r.path); } catch (err) { fail(err); }
}
async function doTrash(entries) {
  if (!entries.length) return;
  try {
    const r = await api("POST", "/api/trash", { paths: entries.map(entryPath) });
    S.sel.clear(); await refresh();
    toast(`🗑️ ${entries.length === 1 ? entries[0].name : entries.length + " items"} moved to trash`, "", {
      label: "Undo", run: async () => {
        try { for (const t of r.trashed) await api("POST", "/api/trash/restore", { root: t.root, id: t.id }); await refresh(); toast("Restored", "ok"); } catch (e) { fail(e); }
      },
    });
  } catch (e) { fail(e); }
}
async function doTransfer(paths, dest, copy) {
  try {
    const r = await api("POST", "/api/transfer", { paths, dest, copy });
    S.sel.clear(); await refresh();
    toast(`${copy ? "⧉ Copied" : "➜ Moved"} ${paths.length === 1 ? baseName(paths[0]) : paths.length + " items"} → ${dest}`, "ok");
    return r;
  } catch (e) { fail(e); }
}
function doDownload(entries) {
  if (entries.length === 1 && entries[0].kind === "file") { location.href = rawUrl(entryPath(entries[0]), true); return; }
  location.href = "/api/zip?" + entries.map((e) => "p=" + enc(entryPath(e))).join("&");
  toast("📦 zipping… the download starts when it's packed");
}
function focusPath(path) {
  const i = S.entries.findIndex((e) => entryPath(e) === path);
  if (i < 0) return;
  S.focus = i; S.anchor = i; S.sel = new Set([path]); paintSel();
  const el = $(`#view [data-i="${i}"]`); if (el) el.scrollIntoView({ block: "nearest" });
}
function copyText(t, what) {
  (navigator.clipboard ? navigator.clipboard.writeText(t) : Promise.reject(new Error("clipboard needs https"))).then(() => toast(`📋 ${what} copied`, "ok"), () => prompt("Copy " + what, "select and copy", t, "Done"));
}

async function showProps(e) {
  const path = entryPath(e);
  let st = e;
  try { st = await api("GET", "/api/stat?path=" + enc(path)); } catch { /* use listing data */ }
  const root = S.roots.find((r) => r.id === path.split("/")[0]);
  const rel = path.split("/").slice(1).join("\\");
  const host = (S.session && S.session.host) || "blackbox";
  const smb = `\\\\${host}\\${root ? root.name : ""}${rel ? "\\" + rel : ""}`;
  const dl = h("dl", { class: "props" });
  const row = (k, v, copy) => dl.append(h("dt", { text: k }), h("dd", {}, copy ? h("span", { class: "copyable", title: "click to copy", text: v, onclick: () => copyText(v, k) }) : v));
  row("name", e.name);
  row("kind", e.kind === "dir" ? "folder" : `${kindOf(e)} · ${e.mime || e.ext || "file"}`);
  if (e.kind !== "dir") row("size", `${fmtSize(e.size)} (${(e.size || 0).toLocaleString()} bytes)`);
  row("modified", new Date((e.mtime || 0) * 1000).toLocaleString());
  if (st.mode) row("permissions", fmtMode(st.mode));
  row("drive", root ? `${root.icon} ${root.name}${root.writable ? "" : " (read-only)"}` : path.split("/")[0]);
  row("on " + host, st.abs || path, true);
  row("samba", smb, true);
  row("smb url", `smb://${host}/${root ? root.name : ""}/${path.split("/").slice(1).join("/")}`, true);
  if (e.link) row("link", e.escapes ? "points outside this drive (blocked)" : "symlink");
  modal(`${iconFor(e)}  ${e.name}`, dl, [h("button", { class: "btn btn-primary", type: "button", text: "Close", onclick: closeModal })]);
}

// folder picker for Move/Copy to…
async function pickFolder(title, okText, startPath) {
  return new Promise((resolve) => {
    let cur = startPath;
    const box = h("div", { class: "picker" });
    const where = h("div", { class: "picker-path" });
    const roots = h("div", { class: "theme-modes", style: { flexWrap: "wrap" } },
      S.roots.filter((r) => r.writable && r.online).map((r) => h("button", { class: "chip", type: "button", text: `${r.icon} ${r.name}`, onclick: () => load(r.id) })));
    const done = (v) => { closeModal(); resolve(v); };
    async function load(p) {
      cur = p; where.textContent = "→ " + p;
      box.replaceChildren(h("div", { class: "loading", text: "…" }));
      try {
        const l = await api("GET", "/api/list?path=" + enc(p));
        const rows = [];
        if (p.includes("/")) rows.push(h("div", { class: "picker-row", onclick: () => load(parentOf(p)) }, "⬆️", h("span", { text: ".. up" })));
        for (const e of l.entries.filter((x) => x.kind === "dir")) rows.push(h("div", { class: "picker-row", onclick: () => load(p + "/" + e.name) }, h("span", { class: "emoji", text: FOLDER_EMOJI[e.name.toLowerCase()] || "📁" }), h("span", { text: e.name })));
        if (!rows.length) rows.push(h("div", { class: "muted", style: { padding: "8px" }, text: "no sub-folders" }));
        box.replaceChildren(...rows);
      } catch (e) { box.replaceChildren(h("div", { class: "muted", text: e.message })); }
    }
    modal(title, h("div", { style: { display: "grid", gap: "10px" } }, roots, where, box),
      [h("button", { class: "btn", type: "button", text: "Cancel", onclick: () => done(null) }),
        h("button", { class: "btn btn-primary", type: "button", text: okText, onclick: () => done(cur) })]);
    load(cur);
  });
}
async function moveOrCopy(entries, copy) {
  const dest = await pickFolder(copy ? "Copy to…" : "Move to…", copy ? "⧉ Copy here" : "➜ Move here", S.path);
  if (dest) doTransfer(entries.map(entryPath), dest, copy);
}

// ── context menu ───────────────────────────────────────────────────────
function closeCtx() { $("#ctx").hidden = true; }
function showCtx(x, y, items) {
  const menu = $("#ctx");
  menu.replaceChildren(...items.filter(Boolean).map((it) => it === "-" ? h("div", { class: "ctx-sep" }) :
    h("button", { class: "ctx-item" + (it.danger ? " danger" : ""), type: "button", role: "menuitem", onclick: () => { closeCtx(); it.run(); } },
      h("span", { class: "emoji", text: it.icon }), h("span", { text: it.label }), it.key ? h("span", { class: "k", text: it.key }) : null)));
  menu.hidden = false;
  const r = menu.getBoundingClientRect();
  menu.style.left = Math.min(x, innerWidth - r.width - 8) + "px";
  menu.style.top = Math.min(y, innerHeight - r.height - 8) + "px";
  menu.querySelector(".ctx-item").focus();
}
function itemMenu(x, y) {
  const sel = selected(); if (!sel.length) return;
  const one = sel.length === 1 ? sel[0] : null;
  const writable = S.listing && S.listing.writable && S.route === "browse";
  showCtx(x, y, [
    one && { icon: one.kind === "dir" ? "📂" : "👁️", label: one.kind === "dir" ? "Open" : "Preview", key: "↵", run: () => openEntry(one) },
    one && S.route === "search" && { icon: "📂", label: "Show in folder", run: () => { const p = entryPath(one); go(parentOf(p)); setTimeout(() => focusPath(p), 400); } },
    { icon: "⬇️", label: sel.length > 1 || (one && one.kind === "dir") ? "Download as zip" : "Download", run: () => doDownload(sel) },
    "-",
    { icon: "⧉", label: "Copy", key: "Ctrl+C", run: () => clipSet("copy") },
    writable && { icon: "✂️", label: "Cut", key: "Ctrl+X", run: () => clipSet("cut") },
    { icon: "📋", label: "Copy to…", run: () => moveOrCopy(sel, true) },
    writable && { icon: "➜", label: "Move to…", run: () => moveOrCopy(sel, false) },
    writable && one && { icon: "✏️", label: "Rename", key: "F2", run: () => doRename(one) },
    writable && one && one.kind === "file" && { icon: "🧬", label: "Duplicate", run: () => doTransfer([entryPath(one)], S.path, true) },
    "-",
    one && { icon: "🔗", label: "Copy Samba path", run: () => { const root = S.roots.find((r) => r.id === entryPath(one).split("/")[0]); copyText(`\\\\${S.session.host}\\${root.name}\\${entryPath(one).split("/").slice(1).join("\\")}`, "Samba path"); } },
    one && { icon: "ⓘ", label: "Properties", key: "I", run: () => showProps(one) },
    writable && "-",
    writable && { icon: "🗑️", label: "Move to trash", key: "Del", danger: true, run: () => doTrash(sel) },
  ]);
}
function bgMenu(x, y) {
  const writable = S.listing && S.listing.writable && S.route === "browse";
  showCtx(x, y, [
    writable && { icon: "⬆️", label: "Upload files", key: "U", run: () => $("#file-input").click() },
    writable && { icon: "📁", label: "New folder", key: "N", run: doMkdir },
    writable && { icon: "📝", label: "New file", run: doNewFile },
    writable && S.clip && { icon: "📋", label: `Paste ${S.clip.paths.length} item${S.clip.paths.length > 1 ? "s" : ""}`, key: "Ctrl+V", run: clipPaste },
    "-",
    { icon: "☑️", label: "Select all", key: "Ctrl+A", run: () => setSel(S.entries.map(entryPath)) },
    { icon: "⬇️", label: "Download folder as zip", run: () => { location.href = "/api/zip?p=" + enc(S.path); } },
    { icon: "🔄", label: "Refresh", key: "R", run: refresh },
  ]);
}

// ── clipboard ──────────────────────────────────────────────────────────
function clipSet(mode) {
  const paths = selected().map(entryPath); if (!paths.length) return;
  S.clip = { mode, paths };
  toast(`${mode === "cut" ? "✂️ Cut" : "⧉ Copied"} ${paths.length} item${paths.length > 1 ? "s" : ""} — Ctrl+V to paste`);
  render();
}
async function clipPaste() {
  if (!S.clip || S.route !== "browse") return;
  const { mode, paths } = S.clip;
  const r = await doTransfer(paths, S.path, mode === "copy");
  if (r && mode === "cut") S.clip = null;
}

// ── uploads ────────────────────────────────────────────────────────────
const U = { queue: [], active: 0, done: 0, failed: 0, total: 0 };
function enqueueUploads(files, dir) {
  if (!files.length) return;
  const panel = $("#uploads"); panel.hidden = false;
  for (const { file, rel } of files) {
    const bar = h("i");
    const pct = h("span", { text: "0%" });
    const item = h("div", { class: "up-item" }, h("div", { class: "up-name" }, h("span", { text: rel }), pct), h("div", { class: "up-bar" }, bar));
    $("#uploads-list").prepend(item);
    U.queue.push({ file, rel, dir, bar, pct, item }); U.total++;
  }
  updateUploadTitle(); pumpUploads();
}
function updateUploadTitle() {
  $("#uploads-title").textContent = `uploads ${U.done}/${U.total}${U.failed ? ` · ${U.failed} failed` : ""}`;
}
function pumpUploads() {
  while (U.active < 3 && U.queue.length) {
    const job = U.queue.shift(); U.active++;
    const xhr = new XMLHttpRequest();
    xhr.open("POST", `/api/upload?dir=${enc(job.dir)}&name=${enc(job.rel)}`);
    xhr.setRequestHeader("X-Holodrive-Csrf", CSRF || "");
    xhr.setRequestHeader("Content-Type", "application/octet-stream");
    xhr.upload.onprogress = (ev) => { if (ev.lengthComputable) { const p = (ev.loaded / ev.total) * 100; job.bar.style.width = p + "%"; job.pct.textContent = p.toFixed(0) + "%"; } };
    xhr.onload = () => {
      U.active--;
      if (xhr.status >= 200 && xhr.status < 300) { U.done++; job.item.classList.add("done"); job.bar.style.width = "100%"; job.pct.textContent = "✓ " + fmtSize(job.file.size); }
      else { U.failed++; job.item.classList.add("fail"); let m = xhr.statusText; try { m = JSON.parse(xhr.responseText).error; } catch { /* plain */ } job.pct.textContent = "✗ " + m; }
      finishUpload();
    };
    xhr.onerror = () => { U.active--; U.failed++; job.item.classList.add("fail"); job.pct.textContent = "✗ network"; finishUpload(); };
    xhr.send(job.file);
  }
}
let refreshTimer = null;
function finishUpload() {
  updateUploadTitle(); pumpUploads();
  clearTimeout(refreshTimer); refreshTimer = setTimeout(refresh, 400);
  if (!U.active && !U.queue.length) {
    toast(U.failed ? `⚠️ ${U.done} uploaded, ${U.failed} failed` : `✅ ${U.done} file${U.done > 1 ? "s" : ""} uploaded`, U.failed ? "err" : "ok");
    if (!U.failed) setTimeout(() => { if (!U.active && !U.queue.length) { $("#uploads").hidden = true; $("#uploads-list").replaceChildren(); U.done = U.total = 0; } }, 4000);
  }
}
async function filesFromDrop(dt) {
  const out = [];
  const items = [...(dt.items || [])].map((i) => i.webkitGetAsEntry && i.webkitGetAsEntry()).filter(Boolean);
  if (!items.length) return [...dt.files].map((f) => ({ file: f, rel: f.name }));
  const walk = async (entry, prefix) => {
    if (entry.isFile) {
      const file = await new Promise((res, rej) => entry.file(res, rej));
      out.push({ file, rel: prefix + file.name });
    } else if (entry.isDirectory) {
      const reader = entry.createReader();
      let batch;
      do {
        batch = await new Promise((res, rej) => reader.readEntries(res, rej));
        for (const e of batch) if (!e.name.startsWith(".")) await walk(e, prefix + entry.name + "/");
      } while (batch.length);
    }
  };
  for (const e of items) await walk(e, "");
  return out;
}

// ── trash & search views ───────────────────────────────────────────────
async function showTrash() {
  S.route = "trash"; S.sel.clear(); renderCrumbs(); markNav(); updateSelbar();
  $("#toolbar").hidden = true;
  const view = $("#view");
  view.replaceChildren(h("div", { class: "loading", text: "scanning trash…" }));
  let t;
  try { t = await api("GET", "/api/trash"); } catch (e) { view.replaceChildren(h("div", { class: "empty", text: e.message })); return; }
  $("#trash-count").textContent = t.items.length ? String(t.items.length) : "";
  if (!t.items.length) {
    view.replaceChildren(h("div", { class: "empty" }, h("div", { class: "empty-icon emoji", text: "♻️" }), h("div", { text: "Trash is empty." }), h("div", { class: "muted", text: `Deleted items stay ${t.days} days, then they're gone for good.` })));
    return;
  }
  const list = h("div", { class: "list trash-list" });
  for (const it of t.items) {
    list.append(h("div", { class: "row" },
      h("span", { class: "row-icon emoji", text: it.kind === "dir" ? "📁" : iconFor({ kind: "file", name: it.name, ext: (it.name.split(".").pop() || "").toLowerCase() }) }),
      h("span", { class: "row-name", title: `${it.root_name}/${it.from}`, text: `${it.name}   ·   ${it.root_icon} ${it.root_name}${it.from ? "/" + it.from : ""}` }),
      h("span", { class: "row-size", text: it.kind === "dir" ? "folder" : fmtSize(it.size) }),
      h("span", { class: "row-date", text: fmtDate(it.deleted) }),
      h("span", { class: "trash-actions" },
        h("button", { class: "btn", type: "button", text: "♻ Restore", onclick: async () => { try { const r = await api("POST", "/api/trash/restore", { root: it.root, id: it.id }); toast("♻️ Restored to " + r.path, "ok"); showTrash(); } catch (e) { fail(e); } } }),
        h("button", { class: "btn btn-danger", type: "button", text: "✕", title: "Delete forever", onclick: async () => { if (await confirmBox("Delete forever?", `“${it.name}” will be gone for good.`, "Delete forever", true)) { try { await api("POST", "/api/trash/purge", { root: it.root, id: it.id }); showTrash(); } catch (e) { fail(e); } } } }))));
  }
  view.replaceChildren(
    h("div", { style: { display: "flex", justifyContent: "space-between", alignItems: "center", gap: "10px", flexWrap: "wrap" } },
      h("div", { class: "section-title", text: `${t.items.length} item${t.items.length > 1 ? "s" : ""} · kept ${t.days} days` }),
      h("button", { class: "btn btn-danger", type: "button", text: "🔥 Empty trash", onclick: async () => {
        if (!(await confirmBox("Empty trash?", `${t.items.length} items will be deleted forever.`, "Empty trash", true))) return;
        for (const it of t.items) await api("POST", "/api/trash/purge", { root: it.root, id: it.id }).catch(fail);
        showTrash();
      } })),
    list);
}
async function showSearch(vpath, q) {
  S.route = "search"; S.path = vpath; S.search = q; S.sel.clear();
  $("#search-input").value = q;
  renderCrumbs(); markNav();
  $("#view").replaceChildren(h("div", { class: "loading", text: `scanning ${vpath} for “${q}”…` }));
  try {
    const r = await api("GET", `/api/search?path=${enc(vpath)}&q=${enc(q)}`);
    S.listing = { path: vpath, writable: false, entries: r.results };
    S.entries = r.results;
    render();
    if (r.truncated) toast("Showing the first results — narrow the search for more.");
  } catch (e) { fail(e); }
}

// ── keyboard ───────────────────────────────────────────────────────────
function columns() {
  const items = $$("#view [data-i]");
  if (S.view === "list" || items.length < 2) return 1;
  const top = items[0].offsetTop;
  const n = items.findIndex((el) => el.offsetTop !== top);
  return n < 0 ? items.length : n;
}
function onKey(ev) {
  const tag = (ev.target.tagName || "").toLowerCase();
  const typing = tag === "input" || tag === "textarea" || tag === "select";
  if (!$("#viewer").hidden) {
    if (ev.key === "Escape") closeViewer();
    else if (ev.key === "ArrowRight" && !typing) viewerStep(1);
    else if (ev.key === "ArrowLeft" && !typing) viewerStep(-1);
    else if (ev.key === "i" && !typing) { const e = V.list[V.i]; if (e) showProps(e); }
    return;
  }
  if (!$("#modal").hidden) { if (ev.key === "Escape") closeModal(); return; }
  if (!$("#ctx").hidden && ev.key === "Escape") { closeCtx(); return; }
  if (typing) { if (ev.key === "Escape") ev.target.blur(); return; }
  if ($("#app").hidden) return;
  const mod = ev.ctrlKey || ev.metaKey;
  const n = S.entries.length;
  const move = (d) => {
    if (!n) return;
    const i = S.focus < 0 ? 0 : Math.max(0, Math.min(n - 1, S.focus + d));
    if (ev.shiftKey) { S.focus = i; clickItem(i, { shiftKey: true }); }
    else { S.focus = i; S.anchor = i; S.sel = new Set([entryPath(S.entries[i])]); paintSel(); }
    const el = $(`#view [data-i="${i}"]`); if (el) el.scrollIntoView({ block: "nearest" });
    ev.preventDefault();
  };
  switch (ev.key) {
    case "ArrowRight": return S.view === "grid" ? move(1) : null;
    case "ArrowLeft": return S.view === "grid" ? move(-1) : null;
    case "ArrowDown": return move(columns());
    case "ArrowUp": return move(-columns());
    case "Home": return move(-n);
    case "End": return move(n);
    case "Enter": if (S.focus >= 0) { ev.preventDefault(); openEntry(S.entries[S.focus]); } return;
    case " ": if (S.focus >= 0 && S.entries[S.focus].kind === "file") { ev.preventDefault(); openEntry(S.entries[S.focus]); } return;
    case "Backspace": if (S.route === "browse" && S.path.includes("/")) { ev.preventDefault(); go(parentOf(S.path)); } return;
    case "Delete": if (S.sel.size && S.listing && S.listing.writable) { ev.preventDefault(); doTrash(selected()); } return;
    case "F2": if (S.sel.size === 1 && S.listing && S.listing.writable) { ev.preventDefault(); doRename(selected()[0]); } return;
    case "Escape": S.sel.clear(); paintSel(); return;
    case "/": ev.preventDefault(); $("#search-input").focus(); return;
  }
  if (mod && ev.key === "a") { ev.preventDefault(); setSel(S.entries.map(entryPath)); return; }
  if (mod && ev.key === "c") { if (S.sel.size) { ev.preventDefault(); clipSet("copy"); } return; }
  if (mod && ev.key === "x") { if (S.sel.size && S.listing.writable) { ev.preventDefault(); clipSet("cut"); } return; }
  if (mod && ev.key === "v") { if (S.clip && S.listing && S.listing.writable) { ev.preventDefault(); clipPaste(); } return; }
  if (mod && ev.key === "k") { ev.preventDefault(); $("#search-input").focus(); return; }
  if (mod || ev.altKey) return;
  const writable = S.listing && S.listing.writable && S.route === "browse";
  if (ev.key === "g") { setView("grid"); } else if (ev.key === "l") { setView("list"); }
  else if (ev.key === "u" && writable) $("#file-input").click();
  else if (ev.key === "n" && writable) doMkdir();
  else if (ev.key === "r") refresh();
  else if (ev.key === "i" && S.sel.size === 1) showProps(selected()[0]);
}
function setView(v) { S.view = v; store.set("view", v); render(); }

// ── wiring ─────────────────────────────────────────────────────────────
function wireApp() {
  document.body.append(h("svg", { width: "0", height: "0", style: { position: "absolute" }, "aria-hidden": "true" },
    h("defs", {}, h("linearGradient", { id: "fold-shine", x1: "0", y1: "0", x2: "0", y2: "1" },
      h("stop", { offset: "0", "stop-color": "#fff", "stop-opacity": ".38" }), h("stop", { offset: "1", "stop-color": "#fff", "stop-opacity": "0" })))));

  const view = $("#view");
  let pressTimer = null, pressed = false;
  view.addEventListener("click", (ev) => {
    const item = ev.target.closest("[data-i]");
    if (ev.target.closest(".row.head")) {
      const k = ev.target.closest("[data-sort]"); if (!k) return;
      if (S.sort === k.dataset.sort) S.dir *= -1; else { S.sort = k.dataset.sort; S.dir = 1; }
      store.set("sort", S.sort); store.set("dir", S.dir); S.entries = sortEntries(S.entries); render(); return;
    }
    if (!item) { if (!ev.shiftKey && !ev.ctrlKey) { S.sel.clear(); paintSel(); } return; }
    if (pressed) { pressed = false; return; }
    const i = +item.dataset.i;
    if (ev.target.closest("[data-check]")) { clickItem(i, { toggle: true }); return; }
    // Click opens (folders navigate, files preview). Selecting is the
    // checkbox, Ctrl/Shift+click, keyboard, or long-press on touch; once
    // something is selected, plain clicks/taps keep toggling.
    if (ev.shiftKey || ev.ctrlKey || ev.metaKey) { clickItem(i, ev); return; }
    if (S.sel.size && (isTouch || ev.target.closest(".selecting"))) { clickItem(i, { toggle: true }); return; }
    S.focus = i; S.anchor = i;
    openEntry(S.entries[i]);
  });
  view.addEventListener("contextmenu", (ev) => {
    ev.preventDefault();
    const item = ev.target.closest("[data-i]");
    if (item) {
      const i = +item.dataset.i;
      if (!S.sel.has(entryPath(S.entries[i]))) clickItem(i, {});
      itemMenu(ev.clientX, ev.clientY);
    } else bgMenu(ev.clientX, ev.clientY);
  });
  // touch: long-press = select (then taps toggle)
  view.addEventListener("touchstart", (ev) => {
    const item = ev.target.closest("[data-i]"); if (!item) return;
    pressTimer = setTimeout(() => { pressed = true; clickItem(+item.dataset.i, { toggle: true }); navigator.vibrate && navigator.vibrate(15); }, 480);
  }, { passive: true });
  for (const t of ["touchend", "touchmove", "touchcancel"]) view.addEventListener(t, () => clearTimeout(pressTimer), { passive: true });

  // drag to move (Ctrl/Alt = copy); files from the desktop = upload
  let dragPaths = null;
  document.addEventListener("dragstart", (ev) => {
    const item = ev.target.closest && ev.target.closest("#view [data-i]"); if (!item) return;
    const e = S.entries[+item.dataset.i];
    if (!S.sel.has(entryPath(e))) clickItem(+item.dataset.i, {});
    dragPaths = selected().map(entryPath);
    ev.dataTransfer.setData("application/x-holodrive", JSON.stringify(dragPaths));
    ev.dataTransfer.effectAllowed = "copyMove";
  });
  document.addEventListener("dragend", () => { dragPaths = null; $$(".drop-target").forEach((el) => el.classList.remove("drop-target")); });
  let dragDepth = 0;
  const isFiles = (ev) => [...(ev.dataTransfer.types || [])].includes("Files");
  document.addEventListener("dragenter", (ev) => {
    if (isFiles(ev) && S.route === "browse" && S.listing && S.listing.writable) { dragDepth++; $("#drop").hidden = false; $("#drop-dest").textContent = "→ " + S.path; }
  });
  document.addEventListener("dragleave", (ev) => { if (isFiles(ev) && --dragDepth <= 0) { dragDepth = 0; $("#drop").hidden = true; } });
  document.addEventListener("dragover", (ev) => {
    const target = ev.target.closest && ev.target.closest("[data-drop]");
    $$(".drop-target").forEach((el) => el !== target && el.classList.remove("drop-target"));
    if (dragPaths && target && !dragPaths.includes(target.dataset.drop)) {
      ev.preventDefault(); target.classList.add("drop-target");
      ev.dataTransfer.dropEffect = ev.ctrlKey || ev.altKey ? "copy" : "move";
    } else if (isFiles(ev)) { ev.preventDefault(); ev.dataTransfer.dropEffect = "copy"; }
  });
  document.addEventListener("drop", async (ev) => {
    ev.preventDefault();
    $("#drop").hidden = true; dragDepth = 0;
    const target = ev.target.closest && ev.target.closest("[data-drop]");
    $$(".drop-target").forEach((el) => el.classList.remove("drop-target"));
    if (dragPaths) {
      if (target && !dragPaths.includes(target.dataset.drop)) doTransfer(dragPaths, target.dataset.drop, ev.ctrlKey || ev.altKey);
      dragPaths = null; return;
    }
    if (isFiles(ev) && S.route === "browse" && S.listing && S.listing.writable) {
      const dest = target && target.closest("#view") ? target.dataset.drop : S.path;
      enqueueUploads(await filesFromDrop(ev.dataTransfer), dest);
    }
  });

  $("#file-input").addEventListener("change", (ev) => { enqueueUploads([...ev.target.files].map((f) => ({ file: f, rel: f.name })), S.path); ev.target.value = ""; });
  $("#upload-btn").addEventListener("click", () => $("#file-input").click());
  $("#fab").addEventListener("click", () => $("#file-input").click());
  $("#mkdir-btn").addEventListener("click", doMkdir);
  $("#newfile-btn").addEventListener("click", doNewFile);
  $("#uploads-close").addEventListener("click", () => { $("#uploads").hidden = true; if (!U.active && !U.queue.length) { $("#uploads-list").replaceChildren(); U.done = U.total = U.failed = 0; } });
  $$(".seg-btn").forEach((b) => b.addEventListener("click", () => setView(b.dataset.view)));
  $("#sort-select").addEventListener("change", (ev) => { S.sort = ev.target.value; store.set("sort", S.sort); S.entries = sortEntries(S.entries); render(); });
  $("#sort-dir").addEventListener("click", () => { S.dir *= -1; store.set("dir", S.dir); S.entries = sortEntries(S.entries); render(); });
  $("#selbar").addEventListener("click", (ev) => {
    const a = ev.target.closest("[data-act]"); if (!a) return;
    const sel = selected();
    ({ download: () => doDownload(sel), copy: () => moveOrCopy(sel, true), move: () => moveOrCopy(sel, false),
      rename: () => sel[0] && doRename(sel[0]), trash: () => doTrash(sel), clear: () => { S.sel.clear(); paintSel(); } })[a.dataset.act]();
  });
  $("#search-form").addEventListener("submit", (ev) => {
    ev.preventDefault();
    const q = $("#search-input").value.trim(); if (!q) return;
    const base = S.route === "trash" ? (S.roots[0] && S.roots[0].id) : S.path.split("/")[0];
    location.hash = `#/search/${encPath(base)}?q=${enc(q)}`;
  });
  $("#nav-toggle").addEventListener("click", (ev) => { ev.stopPropagation(); $("#sidebar").classList.toggle("open"); });
  document.addEventListener("click", (ev) => {
    if (!ev.target.closest("#ctx")) closeCtx();
    if (!ev.target.closest("#sidebar") && !ev.target.closest("#nav-toggle")) $("#sidebar").classList.remove("open");
    if (!ev.target.closest(".dropdown-wrap")) { $("#theme-menu").hidden = true; $("#theme-btn").setAttribute("aria-expanded", "false"); }
  });
  $("#modal-close").addEventListener("click", closeModal);
  $("#modal").addEventListener("click", (ev) => { if (ev.target.id === "modal") closeModal(); });
  $("#viewer-close").addEventListener("click", closeViewer);
  $("#viewer-prev").addEventListener("click", () => viewerStep(-1));
  $("#viewer-next").addEventListener("click", () => viewerStep(1));
  $("#viewer-info").addEventListener("click", () => { const e = V.list[V.i]; if (e) showProps(e); });
  $("#viewer-stage").addEventListener("click", (ev) => { if (ev.target.id === "viewer-stage") closeViewer(); });
  document.addEventListener("keydown", onKey);
  window.addEventListener("scroll", closeCtx, { passive: true });
}

// ── CYBERGRID themes (per device) ──────────────────────────────────────
const T = { catalog: [], appearance: store.get("appearance", "dark"), id: store.get("theme", null) };
async function applyTheme(id) {
  const theme = T.catalog.find((t) => t.id === id) || T.catalog[0];
  if (!theme) return;
  const css = await (await fetch(`/api/cybergrid/css/${enc(theme.id)}?appearance=${T.appearance}`)).text();
  $("#theme-vars").textContent = css;
  $("#theme-label").textContent = theme.name;
  $$("#theme-list .theme-item").forEach((el) => el.classList.toggle("selected", el.dataset.id === theme.id));
  $$("[data-appearance]").forEach((b) => b.classList.toggle("on", b.dataset.appearance === T.appearance));
  document.documentElement.style.colorScheme = T.appearance;
}
async function setupTheme() {
  const r = await (await fetch("/api/cybergrid/themes")).json();
  T.catalog = r.themes;
  const groups = new Map();
  for (const t of T.catalog) { const f = t.family || "Other"; if (!groups.has(f)) groups.set(f, []); groups.get(f).push(t); }
  const list = $("#theme-list");
  list.replaceChildren(...[...groups].flatMap(([fam, themes]) => [
    h("div", { class: "theme-group", text: fam }),
    ...themes.map((t) => h("div", { class: "theme-item", tabindex: "0", role: "option", dataset: { id: t.id, name: t.name.toLowerCase(), fam: fam.toLowerCase() } },
      h("span", { class: "swatch" }, ["bg", "hot_pink", "cyan", "purple", "acid_green"].map((k) => h("i", { style: { background: t.palette[k] } }))),
      h("span", { text: t.name }))),
  ]));
  list.addEventListener("click", (ev) => { const it = ev.target.closest(".theme-item"); if (it) { T.id = it.dataset.id; store.set("theme", T.id); applyTheme(T.id); } });
  list.addEventListener("keydown", (ev) => { if (ev.key === "Enter") { const it = ev.target.closest(".theme-item"); if (it) it.click(); } });
  $("#theme-filter").addEventListener("input", (ev) => {
    const q = ev.target.value.toLowerCase();
    $$("#theme-list .theme-item").forEach((el) => { el.hidden = q && !el.dataset.name.includes(q) && !el.dataset.fam.includes(q); });
    $$("#theme-list .theme-group").forEach((g) => { let n = g.nextElementSibling, any = false; while (n && n.classList.contains("theme-item")) { if (!n.hidden) any = true; n = n.nextElementSibling; } g.hidden = !any; });
  });
  $$("[data-appearance]").forEach((b) => b.addEventListener("click", () => { T.appearance = b.dataset.appearance; store.set("appearance", T.appearance); applyTheme(T.id || r.active); }));
  $("#theme-btn").addEventListener("click", (ev) => {
    ev.stopPropagation();
    const m = $("#theme-menu"); m.hidden = !m.hidden;
    $("#theme-btn").setAttribute("aria-expanded", String(!m.hidden));
    if (!m.hidden) { const s = $("#theme-list .selected"); if (s) s.scrollIntoView({ block: "center" }); }
  });
  await applyTheme(T.id && T.catalog.some((t) => t.id === T.id) ? T.id : r.active);
  // follow the desktop's shared theme until this device picks its own
  try {
    const es = new EventSource("/api/cybergrid/events");
    es.addEventListener("theme-change", async () => {
      if (store.get("theme", null)) return;
      const cur = await (await fetch("/api/cybergrid/themes")).json();
      T.catalog = cur.themes; applyTheme(cur.active);
    });
  } catch { /* no SSE */ }
}

start().catch((e) => { document.body.classList.remove("booting"); toast(e.message, "err"); });
