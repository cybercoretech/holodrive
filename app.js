const root = document.documentElement;
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
