// Holodrive editor bundle: CodeMirror 6 + Vim keys, themed from CYBERGRID
// CSS variables. Built with esbuild into static/editor.js (window.HoloEditor).
import { EditorState, Compartment } from "@codemirror/state";
import { EditorView, keymap, lineNumbers, highlightActiveLine, highlightActiveLineGutter, drawSelection, dropCursor, rectangularSelection, crosshairCursor, highlightSpecialChars } from "@codemirror/view";
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { indentOnInput, bracketMatching, foldGutter, foldKeymap, syntaxHighlighting, HighlightStyle, StreamLanguage, indentUnit } from "@codemirror/language";
import { searchKeymap, highlightSelectionMatches } from "@codemirror/search";
import { autocompletion, completionKeymap, closeBrackets, closeBracketsKeymap } from "@codemirror/autocomplete";
import { vim, Vim, getCM } from "@replit/codemirror-vim";
import { tags as t } from "@lezer/highlight";
import { rust } from "@codemirror/lang-rust";
import { javascript } from "@codemirror/lang-javascript";
import { python } from "@codemirror/lang-python";
import { json } from "@codemirror/lang-json";
import { markdown } from "@codemirror/lang-markdown";
import { html } from "@codemirror/lang-html";
import { css } from "@codemirror/lang-css";
import { yaml } from "@codemirror/lang-yaml";
import { cpp } from "@codemirror/lang-cpp";
import { java } from "@codemirror/lang-java";
import { go } from "@codemirror/lang-go";
import { sql } from "@codemirror/lang-sql";
import { xml } from "@codemirror/lang-xml";
import { toml } from "@codemirror/legacy-modes/mode/toml";
import { shell } from "@codemirror/legacy-modes/mode/shell";
import { lua } from "@codemirror/legacy-modes/mode/lua";
import { dockerFile } from "@codemirror/legacy-modes/mode/dockerfile";
import { properties } from "@codemirror/legacy-modes/mode/properties";
import { diff } from "@codemirror/legacy-modes/mode/diff";
import { ruby } from "@codemirror/legacy-modes/mode/ruby";
import { perl } from "@codemirror/legacy-modes/mode/perl";
import { nginx } from "@codemirror/legacy-modes/mode/nginx";

const legacy = (m) => StreamLanguage.define(m);
const LANG = {
  rs: ["Rust", () => rust()], js: ["JavaScript", () => javascript()], mjs: ["JavaScript", () => javascript()], cjs: ["JavaScript", () => javascript()],
  jsx: ["JSX", () => javascript({ jsx: true })], ts: ["TypeScript", () => javascript({ typescript: true })], tsx: ["TSX", () => javascript({ typescript: true, jsx: true })],
  py: ["Python", () => python()], json: ["JSON", () => json()], jsonc: ["JSON", () => json()], md: ["Markdown", () => markdown()], markdown: ["Markdown", () => markdown()],
  html: ["HTML", () => html()], htm: ["HTML", () => html()], vue: ["HTML", () => html()], svelte: ["HTML", () => html()],
  css: ["CSS", () => css()], scss: ["CSS", () => css()], less: ["CSS", () => css()], yaml: ["YAML", () => yaml()], yml: ["YAML", () => yaml()],
  c: ["C", () => cpp()], h: ["C", () => cpp()], cpp: ["C++", () => cpp()], hpp: ["C++", () => cpp()], cc: ["C++", () => cpp()],
  java: ["Java", () => java()], kt: ["Kotlin", () => java()], go: ["Go", () => go()], sql: ["SQL", () => sql()], xml: ["XML", () => xml()], svg: ["SVG", () => xml()],
  toml: ["TOML", () => legacy(toml)], sh: ["Shell", () => legacy(shell)], bash: ["Shell", () => legacy(shell)], zsh: ["Shell", () => legacy(shell)],
  lua: ["Lua", () => legacy(lua)], dockerfile: ["Dockerfile", () => legacy(dockerFile)], ini: ["INI", () => legacy(properties)], conf: ["Config", () => legacy(properties)],
  cfg: ["Config", () => legacy(properties)], properties: ["Properties", () => legacy(properties)], service: ["systemd", () => legacy(properties)],
  timer: ["systemd", () => legacy(properties)], socket: ["systemd", () => legacy(properties)], desktop: ["Desktop", () => legacy(properties)],
  env: ["Env", () => legacy(properties)], diff: ["Diff", () => legacy(diff)], patch: ["Diff", () => legacy(diff)], rb: ["Ruby", () => legacy(ruby)],
  pl: ["Perl", () => legacy(perl)], nginx: ["nginx", () => legacy(nginx)],
};
const NAMES = { dockerfile: "dockerfile", makefile: "sh", pkgbuild: "sh", ".zshrc": "zsh", ".bashrc": "sh", "cargo.lock": "toml", justfile: "sh" };

function language(name) {
  const lower = name.toLowerCase();
  const ext = NAMES[lower] || (lower.includes(".") ? lower.split(".").pop() : "");
  const l = LANG[ext];
  return l ? { label: l[0], ext: l[1]() } : { label: "Plain text", ext: [] };
}

const theme = EditorView.theme({
  "&": { color: "var(--fg)", backgroundColor: "color-mix(in srgb, var(--bg) 88%, black)", height: "100%", fontSize: "13.5px" },
  ".cm-scroller": { fontFamily: '"JBMono Nerd", var(--font-mono, monospace)', lineHeight: "1.6" },
  ".cm-content": { caretColor: "var(--cyan)", padding: "10px 0" },
  ".cm-cursor, .cm-dropCursor": { borderLeftColor: "var(--cyan)", borderLeftWidth: "2px" },
  ".cm-fat-cursor": { background: "color-mix(in srgb, var(--cyan) 70%, transparent) !important", color: "var(--bg) !important" },
  "&:not(.cm-focused) .cm-fat-cursor": { background: "none !important", outline: "1px solid var(--cyan)" },
  "&.cm-focused .cm-selectionBackground, .cm-selectionBackground, .cm-content ::selection": { backgroundColor: "color-mix(in srgb, var(--pink) 30%, transparent) !important" },
  ".cm-activeLine": { backgroundColor: "color-mix(in srgb, var(--cyan) 6%, transparent)" },
  ".cm-gutters": { backgroundColor: "color-mix(in srgb, var(--panel) 80%, transparent)", color: "var(--muted)", border: "none", borderRight: "1px solid var(--line)" },
  ".cm-activeLineGutter": { backgroundColor: "color-mix(in srgb, var(--pink) 14%, transparent)", color: "var(--fg)" },
  ".cm-foldPlaceholder": { backgroundColor: "var(--panel)", border: "1px solid var(--line)", color: "var(--cyan)" },
  ".cm-matchingBracket, .cm-nonmatchingBracket": { backgroundColor: "color-mix(in srgb, var(--acid) 22%, transparent) !important", outline: "1px solid color-mix(in srgb, var(--acid) 50%, transparent)" },
  ".cm-searchMatch": { backgroundColor: "color-mix(in srgb, var(--orange) 30%, transparent)", outline: "1px solid var(--orange)" },
  ".cm-searchMatch.cm-searchMatch-selected": { backgroundColor: "color-mix(in srgb, var(--pink) 40%, transparent)" },
  ".cm-selectionMatch": { backgroundColor: "color-mix(in srgb, var(--purple) 22%, transparent)" },
  ".cm-panels": { backgroundColor: "var(--panel)", color: "var(--fg)", borderTop: "1px solid var(--line)" },
  ".cm-panels input, .cm-panels button": { fontFamily: "inherit" },
  ".cm-tooltip": { backgroundColor: "var(--panel)", border: "1px solid var(--line)", color: "var(--fg)" },
  ".cm-tooltip-autocomplete > ul > li[aria-selected]": { backgroundColor: "color-mix(in srgb, var(--pink) 25%, transparent)", color: "var(--fg)" },
  ".cm-vim-panel": { backgroundColor: "var(--panel)", color: "var(--fg)", padding: "3px 10px", fontFamily: "inherit" },
  ".cm-vim-panel input": { color: "var(--cyan)", fontFamily: "inherit" },
}, { dark: true });

const highlight = HighlightStyle.define([
  { tag: [t.keyword, t.controlKeyword, t.moduleKeyword, t.operatorKeyword, t.definitionKeyword, t.modifier], color: "var(--pink)" },
  { tag: [t.string, t.special(t.string), t.regexp, t.character], color: "var(--acid)" },
  { tag: [t.number, t.bool, t.null, t.atom, t.unit], color: "var(--orange)" },
  { tag: [t.comment, t.lineComment, t.blockComment, t.docComment], color: "var(--muted)", fontStyle: "italic" },
  { tag: [t.function(t.variableName), t.function(t.propertyName), t.macroName], color: "var(--cyan)" },
  { tag: [t.typeName, t.className, t.namespace, t.standard(t.typeName)], color: "var(--purple)" },
  { tag: [t.propertyName, t.attributeName, t.labelName], color: "color-mix(in srgb, var(--cyan) 70%, var(--fg))" },
  { tag: [t.heading, t.strong], color: "var(--pink)", fontWeight: "bold" },
  { tag: t.emphasis, fontStyle: "italic" },
  { tag: [t.link, t.url], color: "var(--cyan)", textDecoration: "underline" },
  { tag: [t.meta, t.processingInstruction, t.annotation], color: "var(--orange)" },
  { tag: [t.tagName], color: "var(--pink)" },
  { tag: [t.operator, t.punctuation, t.bracket], color: "color-mix(in srgb, var(--fg) 75%, var(--muted))" },
  { tag: t.invalid, color: "var(--red)" },
  { tag: t.inserted, color: "var(--acid)" },
  { tag: t.deleted, color: "var(--red)" },
]);

let exHandlers = null;
Vim.defineEx("write", "w", () => exHandlers && exHandlers.save());
Vim.defineEx("quit", "q", () => exHandlers && exHandlers.quit(false));
Vim.defineEx("wq", "wq", () => exHandlers && exHandlers.save().then((ok) => ok && exHandlers.quit(true)));
Vim.defineEx("xit", "x", () => exHandlers && exHandlers.save().then((ok) => ok && exHandlers.quit(true)));
Vim.map("jk", "<Esc>", "insert");

export function create({ parent, doc, name, readOnly, vimMode, onSave, onQuit, onChange, onStatus }) {
  const lang = language(name);
  const vimC = new Compartment();
  const status = () => {
    const sel = view.state.selection.main;
    const line = view.state.doc.lineAt(sel.head);
    const cm = vimOn ? getCM(view) : null;
    const mode = cm && cm.state.vim ? (cm.state.vim.insertMode ? "INSERT" : cm.state.vim.visualMode ? "VISUAL" : "NORMAL") : "";
    onStatus && onStatus({ line: line.number, col: sel.head - line.from + 1, lines: view.state.doc.lines, mode, lang: lang.label });
  };
  let vimOn = !!vimMode;
  const view = new EditorView({
    parent,
    state: EditorState.create({
      doc,
      extensions: [
        vimC.of(vimOn ? vim({ status: false }) : []),
        lineNumbers(), highlightActiveLineGutter(), highlightSpecialChars(), history(), foldGutter(), drawSelection(), dropCursor(),
        EditorState.allowMultipleSelections.of(true), indentOnInput(), syntaxHighlighting(highlight), bracketMatching(), closeBrackets(),
        autocompletion(), rectangularSelection(), crosshairCursor(), highlightActiveLine(), highlightSelectionMatches(), indentUnit.of("    "),
        keymap.of([
          { key: "Mod-s", preventDefault: true, run: () => { onSave(); return true; } },
          ...closeBracketsKeymap, ...defaultKeymap, ...searchKeymap, ...historyKeymap, ...foldKeymap, ...completionKeymap, indentWithTab,
        ]),
        theme, lang.ext,
        EditorState.readOnly.of(!!readOnly),
        EditorView.updateListener.of((u) => { if (u.docChanged) onChange && onChange(); if (u.docChanged || u.selectionSet || u.focusChanged) status(); }),
        EditorView.domEventHandlers({ keyup: () => { status(); return false; } }),
      ],
    }),
  });
  exHandlers = { save: () => Promise.resolve(onSave()), quit: (force) => onQuit(force) };
  setTimeout(status, 0);
  return {
    view,
    language: lang.label,
    text: () => view.state.doc.toString(),
    setVim(on) { vimOn = on; view.dispatch({ effects: vimC.reconfigure(on ? vim({ status: false }) : []) }); status(); view.focus(); },
    focus: () => view.focus(),
    activate() { exHandlers = { save: () => Promise.resolve(onSave()), quit: (force) => onQuit(force) }; },
    destroy() { view.destroy(); exHandlers = null; },
  };
}
