// The playground page: a screen, the theme's tokens, and the theme file.

import { loadEngine } from "./engine.js";
import { Painter, css } from "./render.js";

const $ = (s) => document.querySelector(s);

const state = {
  engine: null,
  session: null,
  index: null,
  binding: null,
  screen: null,
  palette: null,
  // The theme file being edited. `tokens` and each variant's `tokens`
  // hold what the file sets.
  theme: { "rekindle-theme": 1, id: "dev.you.my-theme", name: "My theme", version: "1.0.0", tokens: {} },
  variant: null,
  // What the engine reported for the current palette.
  values: [],
  recolors: [],
  view: "themed",
  drawables: {},
};

async function main() {
  state.engine = await loadEngine();
  state.index = await (await fetch("fixtures/index.json")).json();
  state.binding = await (await fetch(`fixtures/${state.index.binding}`)).json();
  await loadLocal();
  const pick = $("#screen");
  for (const s of state.index.screens) pick.append(new Option(s.name, s.id));
  pick.onchange = () => openScreen(state.index.screens.find((s) => s.id === pick.value));
  $("#open-capture").onchange = async (e) => {
    const f = e.target.files[0];
    if (f) openCapture(new Uint8Array(await f.arrayBuffer()), { id: f.name, name: f.name, palette: "dark" });
  };
  $("#turn").oninput = () => {
    state.session.replay(+$("#turn").value);
    $("#turn-n").textContent = `${$("#turn").value} / ${state.session.turns}`;
    refresh();
  };
  for (const b of document.querySelectorAll("[data-view]")) b.onclick = () => setView(b.dataset.view);
  $("#import").onchange = async (e) => {
    const f = e.target.files[0];
    if (f) importTheme(JSON.parse(await f.text()));
  };
  $("#export").onclick = exportTheme;
  $("#reset").onclick = () => {
    state.theme.tokens = {};
    for (const v of state.theme.variants || []) v.tokens = {};
    applyTheme();
  };
  for (const k of ["id", "name", "version"]) {
    const el = $(`#theme-${k}`);
    el.value = state.theme[k];
    el.oninput = () => (state.theme[k] = el.value);
  }
  $("#variant").onchange = () => {
    state.variant = $("#variant").value || null;
    applyTheme();
  };
  $("#filter").oninput = () => renderTokens();
  // ?screen=<id>&view=split&theme=<url>&turn=<n> open the page on a
  // screen, a view and a theme file.
  const q = new URLSearchParams(location.search);
  const first = state.index.screens.find((s) => s.id === q.get("screen")) || state.index.screens[0];
  pick.value = first.id;
  if (q.get("view")) setView(q.get("view"));
  await openScreen(first);
  if (q.get("turn")) {
    state.session.replay(+q.get("turn"));
    $("#turn").value = q.get("turn");
    $("#turn-n").textContent = `${q.get("turn")} / ${state.session.turns}`;
  }
  if (q.get("theme")) importTheme(await (await fetch(q.get("theme"))).json());
  if (q.get("variant")) {
    state.variant = q.get("variant");
    $("#variant").value = state.variant;
    applyTheme();
  }
}

// The user's own files in local/ (see the README): drawables and fonts from
// their APK, and captures of their own listed in local/index.json.
async function loadLocal() {
  const get = async (path) => {
    try {
      const r = await fetch(`local/${path}`);
      return r.ok ? await r.json() : null;
    } catch {
      return null;
    }
  };
  state.drawables = (await get("drawables.json")) || {};
  for (const f of (await get("fonts.json")) || []) {
    const face = new FontFace(f.family, `url("local/fonts/${f.file}")`, {
      weight: `${f.weight}`,
      style: f.italic ? "italic" : "normal",
    });
    document.fonts.add(face);
    face.load().catch(() => {});
  }
  const mine = await get("index.json");
  for (const s of mine?.screens || []) state.index.screens.push({ ...s, capture: `../local/${s.capture}`, local: true });
}

async function openScreen(screen) {
  const bytes = new Uint8Array(await (await fetch(`fixtures/${screen.capture}`)).arrayBuffer());
  openCapture(bytes, screen);
}

function openCapture(bytes, screen) {
  state.session?.close();
  state.session = new state.engine.Session(bytes);
  state.screen = screen;
  const pal = state.index.palettes[screen.palette];
  state.palette = pal ? { dark: pal.dark, colors: pal.colors } : null;
  const turn = $("#turn");
  turn.max = state.session.turns;
  turn.value = state.session.turns;
  turn.disabled = state.session.turns < 2;
  $("#turn-n").textContent = `${turn.value} / ${state.session.turns}`;
  for (const f of document.querySelectorAll(".device")) {
    f.style.width = `${screen.width || 412}px`;
    f.style.height = `${screen.height || 915}px`;
  }
  $("#screen-note").textContent = screen.note || "";
  applyTheme();
}

// The tokens the engine is given: the file's top level with the chosen
// variant over it.
function resolvedTokens() {
  const out = { ...state.theme.tokens };
  const v = (state.theme.variants || []).find((v) => v.name === state.variant);
  if (v) Object.assign(out, v.tokens || {});
  return out;
}

function applyTheme() {
  if (!state.session) return;
  const report = state.session.setTheme({ binding: state.binding, palette: state.palette, tokens: resolvedTokens() });
  state.values = report.values;
  state.recolors = report.recolors;
  renderTokens();
  refresh();
}

let frame = 0;
function refresh() {
  cancelAnimationFrame(frame);
  frame = requestAnimationFrame(paint);
}

function paint() {
  const panes = state.view === "split" ? ["guest", "host"] : [state.view === "original" ? "guest" : "host"];
  $("#device-b").parentElement.hidden = panes.length < 2;
  const hosts = [$("#device-a"), $("#device-b")];
  let painter;
  panes.forEach((which, i) => {
    painter = new Painter({ drawables: state.drawables });
    painter.paint(state.session.tree(which === "host"), hosts[i]);
    hosts[i].parentElement.querySelector(".label").textContent = which === "host" ? "With the theme" : "As the client draws it";
  });
  const errors = state.session.errors();
  const kinds = [...painter.kinds.keys()].filter((k) => k.startsWith("(not drawn)"));
  $("#status").textContent = [
    `${state.recolors.length} recolours`,
    `${painter.els.size} nodes`,
    painter.missing.size ? `${painter.missing.size} drawables not loaded` : "",
    kinds.length ? kinds.join(", ") : "",
    errors.length ? `${errors.length} turns refused` : "",
  ]
    .filter(Boolean)
    .join(" · ");
}

function setView(v) {
  state.view = v;
  for (const b of document.querySelectorAll("[data-view]")) b.setAttribute("aria-pressed", b.dataset.view === v);
  refresh();
}

// The token list: each token the build binds, with its colour in this
// screen's palette and the theme's value.
function renderTokens() {
  const list = $("#tokens");
  const set = resolvedTokens();
  const filter = $("#filter").value.trim().toLowerCase();
  list.replaceChildren();
  const groups = { fill: "Surfaces", content: "Text and icons", border: "Borders", any: "Every cell" };
  for (const [role, title] of Object.entries(groups)) {
    const values = state.values.filter((v) => v.role === role && (!filter || v.token.includes(filter)));
    if (!values.length) continue;
    const h = document.createElement("h3");
    h.textContent = title;
    list.append(h);
    for (const v of values) list.append(tokenRow(v, set[v.token]));
  }
  const unbound = Object.keys(set).filter((t) => !state.values.some((v) => v.token === t));
  if (unbound.length) {
    const h = document.createElement("h3");
    h.textContent = "Set, but not bound on this build";
    list.append(h);
    const p = document.createElement("p");
    p.className = "dim";
    p.textContent = unbound.join(", ");
    list.append(p);
  }
}

function tokenRow(v, value) {
  const row = document.createElement("div");
  row.className = value ? "token set" : "token";
  const swatch = document.createElement("input");
  swatch.type = "color";
  swatch.value = (value || v.value).slice(0, 7);
  const name = document.createElement("label");
  name.textContent = v.token;
  name.title = v.live ? "Read from the palette the client named" : "From the binding's table: the client's palette has no key for it";
  const hex = document.createElement("input");
  hex.className = "hex";
  hex.value = value || "";
  hex.placeholder = v.value;
  hex.spellcheck = false;
  const reach = state.recolors.find((r) => r.from === v.value && (r.role === v.role || r.role === "any"));
  const off = document.createElement("button");
  off.textContent = "×";
  off.title = "Use the client's colour";
  off.hidden = !value;
  const set = (c) => {
    writeToken(v.token, c);
    applyTheme();
  };
  swatch.oninput = () => {
    hex.value = swatch.value;
    writeToken(v.token, swatch.value);
    applyThemeSoon();
  };
  swatch.onchange = () => applyTheme();
  hex.onchange = () => {
    const c = hex.value.trim();
    if (!c) set(null);
    else if (/^#?[0-9a-f]{6}([0-9a-f]{2})?$/i.test(c)) set(c.startsWith("#") ? c.toLowerCase() : `#${c.toLowerCase()}`);
  };
  off.onclick = () => set(null);
  row.append(swatch, name, hex, off);
  if (value && !reach) {
    const note = document.createElement("span");
    note.className = "note";
    note.textContent = "Another token of this role has the same colour here and decides.";
    row.append(note);
  }
  return row;
}

let soon = 0;
function applyThemeSoon() {
  cancelAnimationFrame(soon);
  soon = requestAnimationFrame(applyTheme);
}

// An edit goes to the chosen variant when the variant sets the token, and
// to the top level otherwise.
function writeToken(token, color) {
  const v = (state.theme.variants || []).find((v) => v.name === state.variant);
  const layer = v && v.tokens && token in v.tokens ? v.tokens : state.theme.tokens;
  if (color) layer[token] = color;
  else {
    delete layer[token];
    if (v?.tokens) delete v.tokens[token];
  }
}

function importTheme(t) {
  if (t["rekindle-theme"] !== 1) {
    alert("Not a rekindle theme file: it has no \"rekindle-theme\": 1.");
    return;
  }
  state.theme = t;
  t.tokens = t.tokens || {};
  for (const k of ["id", "name", "version"]) $(`#theme-${k}`).value = t[k] || "";
  const sel = $("#variant");
  sel.replaceChildren(new Option("(none)", ""));
  for (const v of t.variants || []) sel.append(new Option(v.label || v.name, v.name));
  state.variant = t["default-variant"] || t.variants?.[0]?.name || null;
  sel.value = state.variant || "";
  sel.disabled = !(t.variants || []).length;
  applyTheme();
}

// The theme file as it is exported.
function themeFile() {
  const t = structuredClone(state.theme);
  for (const v of t.variants || []) if (v.tokens && !Object.keys(v.tokens).length) delete v.tokens;
  return `${JSON.stringify(t, null, 2)}\n`;
}

function exportTheme() {
  const t = state.theme;
  const text = themeFile();
  const a = document.createElement("a");
  a.href = URL.createObjectURL(new Blob([text], { type: "application/json" }));
  const slug = (t.id || "theme").split(".").pop();
  a.download = `${slug}.theme.json`;
  a.click();
  URL.revokeObjectURL(a.href);
}

// For tests and the console.
window.playground = { state, applyTheme, importTheme, writeToken, themeFile, css };

main().catch((e) => {
  $("#status").textContent = `Failed to start: ${e.message}`;
  console.error(e);
});
