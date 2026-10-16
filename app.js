const root = document.documentElement;
const picker = document.querySelector("#themeSelect");
const button = document.querySelector("#themeButton");
const menu = document.querySelector("#themeMenu");
const labels = {
  "cybercore-tech": "CYBERCORE TECH / MASTER MIX",
  cyberdyne: "CYBERDYNE",
  cyberpunk: "CYBERPUNK",
  default: "DEFAULT / CLASSICS",
  dystopian: "DYSTOPIAN",
  neosynth: "NEOSYNTH",
  "omarchy-live": "OMARCHY LIVE",
  synthwave: "SYNTHWAVE",
};
const display = (name) => name.replaceAll("-", " ").toUpperCase();
const paletteCache = new Map();
const catalogUrl = "https://cybercore-tech.github.io/data/cybergrid.json";

function applyPalette(name, palette) {
  const variables = {
    bg: "--bg", white: "--white", acid_green: "--acid", hot_pink: "--pink",
    purple: "--purple", cyan: "--cyan", orange: "--orange", red: "--red",
    panel: "--panel", line: "--line", muted: "--muted",
  };
  for (const [key, variable] of Object.entries(variables)) {
    if (palette[key]) root.style.setProperty(variable, `#${palette[key].replace(/^#/, "")}`);
  }
  root.dataset.theme = name;
  try { localStorage.setItem("holodrive-site-theme", name); } catch { /* storage may be disabled */ }
  syncThemeMenu();
}

async function loadPalette(name, family) {
  const key = `${family}/${name}`;
  if (!paletteCache.has(key)) {
    const url = `https://cybercore-tech.github.io/data/themes/${encodeURIComponent(family)}/${encodeURIComponent(name)}.json`;
    paletteCache.set(key, fetch(url).then((response) => {
      if (!response.ok) throw new Error(`Theme request failed (${response.status})`);
      return response.json();
    }));
  }
  return paletteCache.get(key);
}

function syncThemeMenu() {
  const selected = picker.selectedOptions[0];
  button.textContent = selected?.textContent || "THEMES OFFLINE";
  menu.querySelectorAll(".theme-option").forEach((option) => {
    option.setAttribute("aria-selected", String(option.dataset.value === picker.value));
  });
}

function closeMenu() {
  menu.hidden = true;
  button.setAttribute("aria-expanded", "false");
}

function buildMenu() {
  menu.replaceChildren();
  for (const group of picker.querySelectorAll("optgroup")) {
    const heading = document.createElement("div");
    heading.className = "theme-group-label";
    heading.textContent = group.label;
    menu.append(heading);
    for (const source of group.querySelectorAll("option")) {
      const option = document.createElement("button");
      option.type = "button";
      option.className = "theme-option";
      option.setAttribute("role", "option");
      option.dataset.value = source.value;
      option.textContent = source.textContent;
      option.addEventListener("click", () => {
        picker.value = source.value;
        picker.dispatchEvent(new Event("change"));
        closeMenu();
        button.focus();
      });
      menu.append(option);
    }
  }
  syncThemeMenu();
}

async function bootThemes() {
  const response = await fetch(catalogUrl);
  if (!response.ok) throw new Error(`Theme catalog request failed (${response.status})`);
  const catalog = await response.json();
  for (const [family, names] of Object.entries(catalog.families)) {
    const group = document.createElement("optgroup");
    group.label = labels[family] || family.toUpperCase();
    for (const name of names) {
      const option = document.createElement("option");
      option.value = name;
      option.textContent = display(name);
      option.dataset.family = family;
      group.append(option);
    }
    picker.append(group);
  }
  buildMenu();
  const params = new URLSearchParams(location.search);
  const requested = params.get("theme");
  let saved = null;
  try { saved = localStorage.getItem("holodrive-site-theme"); } catch { /* use the catalog default */ }
  const available = Object.entries(catalog.families).find(([, names]) => names.includes(requested || saved || catalog.active));
  const family = available?.[0] || "cybercore-tech";
  const name = available?.[1].includes(requested || saved || catalog.active) ? (requested || saved || catalog.active) : catalog.active;
  picker.value = name;
  applyPalette(name, await loadPalette(name, family));
  picker.addEventListener("change", async () => {
    const option = picker.selectedOptions[0];
    const palette = await loadPalette(option.value, option.dataset.family);
    applyPalette(option.value, palette);
    history.replaceState(null, "", `?theme=${encodeURIComponent(option.value)}`);
  });
}

button.addEventListener("click", () => {
  const opening = menu.hidden;
  menu.hidden = !opening;
  button.setAttribute("aria-expanded", String(opening));
  if (opening) menu.querySelector('[aria-selected="true"]')?.focus();
});
document.addEventListener("pointerdown", (event) => {
  if (!event.target.closest(".theme-picker")) closeMenu();
});
document.addEventListener("keydown", (event) => {
  if (event.key === "Escape" && !menu.hidden) {
    closeMenu();
    button.focus();
  }
});

bootThemes().catch((error) => {
  console.error("Cybercore themes could not load:", error);
  button.textContent = "THEME CATALOG OFFLINE";
  const fallback = document.createElement("optgroup");
  fallback.label = "CYBERCORE TECH";
  const option = document.createElement("option");
  option.value = "cybercore-tech";
  option.textContent = "CYBERCORE TECH";
  option.dataset.family = "cybercore-tech";
  fallback.append(option);
  picker.append(fallback);
  buildMenu();
});

if (!matchMedia("(prefers-reduced-motion: reduce)").matches) {
  window.addEventListener("pointermove", (event) => {
    root.style.setProperty("--x", `${event.clientX}px`);
    root.style.setProperty("--y", `${event.clientY}px`);
  }, { passive: true });
}

// Copy individual release commands without requiring a third-party library.
async function copyCommandText(value) {
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(value);
      return true;
    }
  } catch { /* Try the selection-based fallback below. */ }
  const field = document.createElement("textarea");
  field.value = value;
  field.setAttribute("readonly", "");
  field.style.position = "fixed";
  field.style.opacity = "0";
  document.body.append(field);
  field.select();
  const copied = document.execCommand("copy");
  field.remove();
  return copied;
}

document.querySelectorAll("[data-copy-command]").forEach((control) => {
  control.addEventListener("click", async () => {
    const command = control.closest(".command-line")?.querySelector("code")?.textContent?.trim();
    if (!command) return;
    const copied = await copyCommandText(command);
    control.textContent = copied ? "COPIED" : "FAILED";
    control.dataset.state = copied ? "copied" : "failed";
    control.setAttribute("aria-label", copied ? "Command copied to clipboard" : "Could not copy command");
    window.setTimeout(() => {
      control.textContent = "COPY";
      delete control.dataset.state;
      control.setAttribute("aria-label", "Copy command to clipboard");
    }, 1800);
  });
});
