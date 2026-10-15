// The page behind a share link (/s/<key>): one file, a folder to browse and
// download, or a drop box to upload into. No session, no app shell.
"use strict";
const KEY = location.pathname.split("/").filter(Boolean)[1] || "";
const API = `/api/s/${KEY}`;
const $ = (s) => document.querySelector(s);
const enc = encodeURIComponent;
function h(tag, props, ...kids) {
  const el = document.createElement(tag);
  for (const [k, v] of Object.entries(props || {})) {
    if (k === "text") el.textContent = v;
    else if (k === "class") el.className = v;
    else if (k.startsWith("on")) el.addEventListener(k.slice(2), v);
    else if (v !== false && v != null) el.setAttribute(k, v === true ? "" : v);
  }
  for (const c of kids.flat()) if (c != null) el.append(c);
  return el;
}
function fmtSize(n) {
  if (n == null) return "";
  const u = ["B", "KB", "MB", "GB", "TB"]; let i = 0;
  while (n >= 1024 && i < u.length - 1) { n /= 1024; i++; }
  return (i ? n.toFixed(n < 10 ? 1 : 0) : n) + " " + u[i];
}
const fmtDate = (s) => new Date(s * 1000).toLocaleString();
const ICON = { dir: "📁", image: "🖼️", video: "🎬", audio: "🎵", pdf: "📕", archive: "🗜️" };
function iconFor(e) {
  if (e.kind === "dir") return ICON.dir;
  const m = e.mime || "";
  if (m.startsWith("image/")) return ICON.image;
  if (m.startsWith("video/")) return ICON.video;
  if (m.startsWith("audio/")) return ICON.audio;
  if (m === "application/pdf") return ICON.pdf;
  if (/zip|tar|7z|rar|gz/.test(e.ext || "")) return ICON.archive;
  return "📄";
}
async function get(url, opts) {
  const r = await fetch(url, { credentials: "same-origin", ...opts });
  const data = await r.json().catch(() => ({}));
  if (!r.ok) { const e = new Error(data.error || r.statusText); e.status = r.status; e.locked = data.locked; throw e; }
  return data;
}
const show = (...kids) => $("#body").replaceChildren(...kids);
const fail = (msg) => show(h("div", { class: "link-head" }, h("span", { class: "emoji", style: "font-size:34px", text: "🔒" }),
  h("div", {}, h("div", { class: "link-name", text: "Link unavailable" }), h("div", { class: "link-err", text: msg }))));
function head(info, sub) {
  const bits = [];
  if (info.size != null) bits.push(fmtSize(info.size));
  if (info.expires) bits.push("until " + fmtDate(info.expires));
  if (info.downloads_left != null) bits.push(`${info.downloads_left} download${info.downloads_left === 1 ? "" : "s"} left`);
  return h("div", { class: "link-head" }, h("img", { src: "/static/holocube.svg", width: "44", height: "44", alt: "" }),
    h("div", {}, h("div", { class: "link-name", text: info.label || info.name }),
      h("div", { class: "link-meta", text: [sub, ...bits].filter(Boolean).join(" · ") })));
}

async function start() {
  if (!/^[0-9a-f]{32}$/.test(KEY)) return fail("This link isn't valid.");
  let info;
  try { info = await get(API); } catch (e) { return fail(e.message); }
  if (info.locked) return askPassword();
  document.title = `${info.label || info.name} · HOLODRIVE`;
  if (info.mode === "drop") return dropBox(info);
  if (info.dir) return folder(info, "");
  return file(info);
}

function askPassword(msg) {
  const input = h("input", { class: "text-input", type: "password", autocomplete: "off", placeholder: "password" });
  const err = h("div", { class: "link-err", text: msg || "" });
  const form = h("form", { class: "field", onsubmit: async (ev) => {
    ev.preventDefault();
    try { await get(`${API}/unlock`, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ password: input.value }) }); start(); }
    catch (e) { err.textContent = e.message; input.select(); }
  } }, h("span", { class: "field-label", text: "this link needs a password" }), input, err,
  h("button", { class: "btn btn-primary btn-wide", type: "submit", text: "Open" }));
  show(h("div", { class: "link-head" }, h("img", { src: "/static/holocube.svg", width: "44", height: "44", alt: "" }), h("div", { class: "link-name", text: "Protected link" })), form);
  setTimeout(() => input.focus(), 30);
}

function file(info) {
  const raw = `${API}/raw`;
  const kids = [head(info, "shared file")];
  const ext = (info.name.split(".").pop() || "").toLowerCase();
  if (/^(jpe?g|png|gif|webp|avif|bmp)$/.test(ext)) kids.push(h("img", { class: "link-preview", src: raw, alt: info.name }));
  else if (/^(mp4|webm|mov|m4v|ogv)$/.test(ext)) kids.push(h("video", { class: "link-preview", src: raw, controls: true, preload: "metadata" }));
  else if (/^(mp3|flac|ogg|oga|wav|m4a|opus|aac)$/.test(ext)) kids.push(h("audio", { src: raw, controls: true, preload: "metadata" }));
  kids.push(h("a", { class: "btn btn-primary btn-wide", href: `${raw}?dl=1`, text: "⬇ Download" }));
  show(...kids);
}

async function folder(info, sub) {
  let data;
  try { data = await get(`${API}/list?p=${enc(sub)}`); } catch (e) { if (e.locked) return askPassword(); return fail(e.message); }
  const parts = sub ? sub.split("/") : [];
  const crumbs = h("div", { class: "link-crumbs" },
    h("button", { type: "button", text: info.name, onclick: () => folder(info, "") }),
    parts.map((p, i) => [h("span", { class: "muted", text: "/" }), h("button", { type: "button", text: p, onclick: () => folder(info, parts.slice(0, i + 1).join("/")) })]));
  const rows = data.entries.map((e) => {
    const path = sub ? `${sub}/${e.name}` : e.name;
    const open = e.kind === "dir"
      ? h("button", { class: "n", type: "button", text: e.name, onclick: () => folder(info, path) })
      : h("a", { class: "n", href: `${API}/raw?p=${enc(path)}`, target: "_blank", rel: "noopener", text: e.name });
    const dl = e.kind === "dir"
      ? h("a", { class: "btn-ghost", href: `${API}/zip?p=${enc(path)}`, title: "download as zip", text: "⬇ zip" })
      : h("a", { class: "btn-ghost", href: `${API}/raw?p=${enc(path)}&dl=1`, title: "download", text: "⬇" });
    return h("div", { class: "link-row" }, h("span", { class: "emoji", text: iconFor(e) }), open,
      h("span", { class: "muted", text: e.kind === "dir" ? "" : fmtSize(e.size) }), dl);
  });
  show(head(info, "shared folder"), crumbs,
    rows.length ? h("div", { class: "link-list" }, rows) : h("div", { class: "muted", text: "This folder is empty." }),
    h("a", { class: "btn btn-primary btn-wide", href: `${API}/zip?p=${enc(sub)}`, text: `⬇ Download ${sub ? parts[parts.length - 1] : "everything"} as zip` }));
}

function dropBox(info) {
  const list = h("div", { class: "field" });
  const picker = h("input", { type: "file", multiple: true, hidden: true, onchange: () => { send([...picker.files]); picker.value = ""; } });
  const zone = h("div", { class: "drop-zone", tabindex: "0", onclick: () => picker.click(),
    onkeydown: (ev) => { if (ev.key === "Enter" || ev.key === " ") { ev.preventDefault(); picker.click(); } },
    ondragover: (ev) => { ev.preventDefault(); zone.classList.add("over"); },
    ondragleave: () => zone.classList.remove("over"),
    ondrop: (ev) => { ev.preventDefault(); zone.classList.remove("over"); send([...ev.dataTransfer.files]); } },
  h("div", { class: "emoji", style: "font-size:34px", text: "📥" }),
  h("div", { text: "Drop files here, or tap to choose" }),
  h("div", { class: "muted", text: "They go straight to the owner. You can't see what's already there." }));
  show(head(info, "drop box"), zone, picker, list);
  function send(files) {
    for (const f of files) {
      const bar = h("i"), pct = h("span", { class: "muted", text: "0%" });
      list.prepend(h("div", { class: "drop-item" }, h("span", { text: f.name }), pct, h("div", { class: "drop-bar" }, bar)));
      const xhr = new XMLHttpRequest();
      xhr.open("POST", `${API}/upload?name=${enc(f.name)}`);
      xhr.setRequestHeader("Content-Type", "application/octet-stream");
      xhr.upload.onprogress = (ev) => { if (ev.lengthComputable) { const p = ev.loaded / ev.total * 100; bar.style.width = p + "%"; pct.textContent = p.toFixed(0) + "%"; } };
      xhr.onload = () => {
        if (xhr.status >= 200 && xhr.status < 300) { bar.style.width = "100%"; pct.textContent = "✓ " + fmtSize(f.size); return; }
        let m = xhr.statusText; try { m = JSON.parse(xhr.responseText).error; } catch { /* plain */ }
        pct.textContent = "✗ " + m; pct.className = "link-err";
      };
      xhr.onerror = () => { pct.textContent = "✗ network"; pct.className = "link-err"; };
      xhr.send(f);
    }
  }
}

start();
