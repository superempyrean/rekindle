// A Phoenix wire tree painted with the DOM.
//
// This is phoenix-view's layout (public/client/phoenix-view/src/view.rs,
// `render_node`) written for CSS flexbox: a `view` is a flex column unless
// `direction` is "row", a `stack` layers its children over the first, a
// `navigator` shows the child its `index` names, and one dp is one CSS
// pixel. Items keep their size unless `flex` or `"fill"` says otherwise,
// and their minimum is 0, as Yoga lays out Android's views.

export function css(argb) {
  if (argb === undefined || argb === null || typeof argb !== "number") return null;
  const c = argb >>> 0;
  const a = (c >>> 24) / 255;
  return `rgba(${(c >>> 16) & 255},${(c >>> 8) & 255},${c & 255},${+a.toFixed(3)})`;
}

const num = (v) => (typeof v === "number" && Number.isFinite(v) ? v : null);
const str = (v) => (typeof v === "string" ? v : null);
const quad = (v) => (v && Array.isArray(v.$quad) ? v.$quad : null);
const percent = (s) => (typeof s === "string" && s.endsWith("%") ? parseFloat(s) : null);

// What a box sits in, which decides what "fill" means for it.
const ROW = "row", COLUMN = "column", VIEWPORT = "viewport", LAYER = "layer";

const OVERLAY_KINDS = new Set(["modal", "systemOverlay", "popover"]);
const BOX_KINDS = new Set(["view", "flow", "sheet", "modal", "banner", "popover", "systemOverlay", "zoom"]);

export class Painter {
  // `drawables`: name -> {uri, width, height} in dp, from the user's own
  // APK (see the README). Missing ones are drawn as a box of their tint.
  constructor({ drawables = {}, fonts = {} } = {}) {
    this.drawables = drawables;
    this.fonts = fonts;
    this.missing = new Set();
    this.kinds = new Map();
  }

  // Paint `tree` (Session.tree()) into `host`, replacing what was there.
  paint(tree, host) {
    this.tree = tree;
    this.popovers = [];
    this.missing.clear();
    this.kinds.clear();
    this.els = new Map();
    host.replaceChildren();
    if (tree.root == null || !tree.nodes[tree.root]) return;
    const root = this.node(tree.root, VIEWPORT);
    root.style.width = "100%";
    root.style.height = "100%";
    host.append(root);
    for (const id of this.popovers) this.popover(id, host);
    // Lists that stick to their end start scrolled there, as a chat does.
    for (const el of host.querySelectorAll("[data-stick]")) el.scrollTop = el.scrollHeight;
  }

  get(id) {
    return this.tree.nodes[id];
  }

  overlays(id) {
    const n = this.get(id);
    return !!n && (OVERLAY_KINDS.has(n.k) || n.p.absolute === true);
  }

  node(id, parent) {
    const n = this.get(id);
    const el = document.createElement("div");
    if (!n) return el;
    const p = n.p;
    this.kinds.set(n.k, (this.kinds.get(n.k) || 0) + 1);
    if (p.hidden === true) {
      el.style.display = "none";
      return el;
    }
    if (n.k === "popover" && !this.hoisting) {
      this.popovers.push(id);
      el.style.cssText = "position:absolute;width:0;height:0";
      return el;
    }
    this.hoisting = false;
    el.dataset.node = id;
    el.dataset.kind = n.k;
    this.els.set(+id, el);
    const s = el.style;
    s.position = "relative";
    s.flexShrink = "0";
    s.minWidth = "0";
    s.minHeight = "0";
    if (n.k === "modal" || n.k === "systemOverlay") {
      s.width = "100%";
      s.height = "100%";
    }

    // The box: size, flex, spacing.
    const fillW = p.width === "fill", fillH = p.height === "fill";
    if (fillW) {
      if (parent === ROW) Object.assign(s, { flexGrow: "1", flexShrink: "1", flexBasis: "0px" });
      else s.alignSelf = "stretch";
    }
    if (parent === VIEWPORT) s.flexGrow = "1";
    if (fillH) {
      if (parent === COLUMN) Object.assign(s, { flexGrow: "1", flexShrink: "1", flexBasis: "0px" });
      else if (parent === ROW) s.alignSelf = "stretch";
      else if (parent === LAYER) Object.assign(s, { flexGrow: "1", flexShrink: "1" });
    }
    if (num(p.width) != null) s.width = `${p.width}px`;
    if (num(p.height) != null) s.height = `${p.height}px`;
    if (percent(p.width) != null) s.width = p.width;
    if (percent(p.height) != null) s.height = p.height;
    if (num(p.flex) != null) Object.assign(s, { flexGrow: `${p.flex}`, flexShrink: "1", flexBasis: "0px" });
    if (p.flexBasis === "auto") s.flexBasis = "auto";
    else if (num(p.flexBasis) != null) s.flexBasis = `${p.flexBasis}px`;
    if (num(p.minWidth) != null) s.minWidth = `${p.minWidth}px`;
    if (num(p.minHeight) != null) s.minHeight = `${p.minHeight}px`;
    if (num(p.maxWidth) != null) s.maxWidth = `${p.maxWidth}px`;
    if (num(p.maxHeight) != null) s.maxHeight = `${p.maxHeight}px`;
    const pad = quad(p.padding);
    if (pad) s.padding = `${pad[1]}px ${pad[2]}px ${pad[3]}px ${pad[0]}px`;
    const mar = quad(p.margin);
    if (mar) s.margin = `${mar[1]}px ${mar[2]}px ${mar[3]}px ${mar[0]}px`;
    if (num(p.gap) != null) s.gap = `${p.gap}px`;
    if (p.shrink === true) s.flexShrink = "1";
    if (num(p.zIndex) != null) s.zIndex = `${p.zIndex}`;

    // The paint: the colours a press look paints at rest stand over the
    // node's own cells.
    const rest = n.rest || {};
    const bg = rest.background ?? p.background;
    if (css(bg)) s.backgroundColor = css(bg);
    else if (p.blur && typeof p.blur === "object" && css(p.blur.fallback)) s.backgroundColor = css(p.blur.fallback);
    const grad = gradient(p.backgroundGradient, p.backgroundGradientAxis);
    if (grad) s.backgroundImage = grad;
    if (p.shadow && typeof p.shadow === "object") s.boxShadow = shadow(p.shadow);
    const radius = num(p.radius);
    if (radius != null) s.borderRadius = `${radius}px`;
    const corners = [p.radiusTopLeft, p.radiusTopRight, p.radiusBottomRight, p.radiusBottomLeft];
    const cornerProps = ["borderTopLeftRadius", "borderTopRightRadius", "borderBottomRightRadius", "borderBottomLeftRadius"];
    corners.forEach((r, i) => {
      if (num(r) != null) s[cornerProps[i]] = `${r}px`;
    });
    const outline = p.clipOutline && typeof p.clipOutline === "object" ? p.clipOutline : null;
    if (outline) {
      if (outline.circle === true) s.borderRadius = "9999px";
      else if (num(outline.radius) > 0) s.borderRadius = `${outline.radius}px`;
      s.overflow = "hidden";
      // A hole is cut out of the node, as Android clips the avatar around
      // its status badge.
      if (Array.isArray(outline.holes) && outline.holes.length) {
        const masks = outline.holes
          .filter((h) => Array.isArray(h) && h.length >= 3)
          .map(([x, y, r]) => `radial-gradient(circle ${r}px at ${x}px ${y}px, transparent ${r}px, #000 ${r + 0.5}px)`);
        if (masks.length) {
          s.maskImage = masks.join(",");
          s.maskComposite = "intersect";
          s.webkitMaskImage = masks.join(",");
        }
      }
    }
    const bw = num(p.borderWidth);
    if (bw != null) {
      const sides = Array.isArray(p.borderSides) && p.borderSides.length === 4 ? p.borderSides.map((v) => !(num(v) === 0)) : [true, true, true, true];
      const names = ["borderTop", "borderRight", "borderBottom", "borderLeft"];
      const color = css(rest.borderColor ?? p.borderColor) || "transparent";
      sides.forEach((on, i) => {
        if (on) s[names[i]] = `${bw}px solid ${color}`;
      });
      el.dataset.border = "1";
    }
    if (num(p.opacity) != null) s.opacity = `${Math.min(1, Math.max(0, p.opacity))}`;
    if (p.clip === true) s.overflow = "hidden";
    const tx = num(p.translateX) || 0, ty = num(p.translateY) || 0;
    const sc = num(p.scaleFactor);
    const transforms = [];
    if (tx || ty) transforms.push(`translate(${tx}px,${ty}px)`);
    if (sc != null && Math.abs(sc - 1) > 1e-3 && n.k === "image") transforms.push(`scale(${sc})`);
    if (transforms.length) s.transform = transforms.join(" ");
    if (Array.isArray(p.on) && p.on.includes("press")) s.cursor = "pointer";

    const row = p.direction === "row";
    const flexbox = () => {
      s.display = "flex";
      s.flexDirection = row ? "row" : "column";
      s.alignItems = { center: "center", end: "flex-end", stretch: "stretch" }[p.align] || "flex-start";
      s.justifyContent =
        { center: "center", end: "flex-end", between: "space-between", around: "space-around", evenly: "space-evenly" }[p.justify] ||
        "flex-start";
    };
    const axis = row ? ROW : COLUMN;

    switch (n.k) {
      case "text":
      case "markup":
        this.text(n, el, rest);
        break;
      case "image":
        this.image(n, el, rest);
        break;
      case "input":
        this.input(n, el);
        break;
      case "stack": {
        s.display = "flex";
        s.flexDirection = "column";
        const flow = n.c.find((c) => !this.overlays(c));
        for (const c of n.c) {
          const child = this.node(c, LAYER);
          el.append(c === flow ? child : layer(child, str(this.get(c)?.p.gravity)));
        }
        break;
      }
      case "navigator": {
        s.display = "flex";
        s.flexDirection = "column";
        this.screens(id).forEach((c, i) => {
          const screen = document.createElement("div");
          screen.style.cssText = "width:100%;height:100%;display:flex;flex-direction:column";
          if (i > 0) Object.assign(screen.style, { position: "absolute", top: "0", left: "0" });
          screen.append(this.node(c, LAYER));
          el.append(screen);
        });
        break;
      }
      case "scroll":
      case "list":
      case "hscroll": {
        const horizontal = n.k === "hscroll";
        s.display = "flex";
        s.flexDirection = "column";
        s.overflow = "hidden";
        const port = document.createElement("div");
        port.className = horizontal ? "port h" : "port";
        if (!horizontal && p.stickToEnd === true) port.dataset.stick = "1";
        const content = document.createElement("div");
        content.style.cssText = `display:flex;flex-shrink:0;flex-direction:${horizontal ? "row" : "column"};${horizontal ? "min-width:100%" : "min-height:100%"}`;
        const inner = horizontal ? ROW : n.c.length === 1 ? VIEWPORT : COLUMN;
        for (const c of n.c) {
          if (this.overlays(c)) el.append(layer(this.node(c, LAYER), str(this.get(c)?.p.gravity)));
          else content.append(this.node(c, inner));
        }
        // The list's padding belongs to its content, which scrolls.
        if (pad) {
          s.padding = "0";
          content.style.padding = `${pad[1]}px ${pad[2]}px ${pad[3]}px ${pad[0]}px`;
        }
        port.append(content);
        el.append(port);
        break;
      }
      default:
        if (!BOX_KINDS.has(n.k)) this.kinds.set(`(not drawn) ${n.k}`, 1);
        flexbox();
        for (const c of n.c) {
          if (this.overlays(c)) el.append(layer(this.node(c, LAYER), str(this.get(c)?.p.gravity)));
          else el.append(this.node(c, axis));
        }
    }
    return el;
  }

  // The screens a navigator shows: with `transition: "layers"`, every
  // child the guest hasn't put away, else the one `index` names.
  screens(id) {
    const n = this.get(id);
    const pick = () => {
      const i = num(n.p.index) || 0;
      const c = n.c[i] ?? n.c[n.c.length - 1];
      return c == null ? [] : [c];
    };
    if (n.p.transition !== "layers") return pick();
    const marked = (c) => {
      const p = this.get(c)?.p;
      return !!p && p.accessibilityHidden === true && p.pointerEvents === "none";
    };
    const putAway = (c) => marked(c) || (this.get(c)?.c.length === 1 && marked(this.get(c).c[0]));
    const shown = n.c.filter((c) => !putAway(c));
    return shown.length ? shown : pick();
  }

  text(n, el, rest) {
    const p = n.p;
    const s = el.style;
    s.display = "block";
    if (num(p.maxWidth) == null) s.maxWidth = "100%";
    const color = rest.color ?? p.color;
    if (css(color)) s.color = css(color);
    s.fontSize = `${num(p.fontSize) ?? 16}px`;
    if (num(p.fontWeight) != null) s.fontWeight = `${p.fontWeight}`;
    if (num(p.lineHeight) != null) s.lineHeight = `${p.lineHeight}px`;
    s.fontFamily = p.fontFamily === "ginto" ? "var(--font-display)" : "var(--font-sans)";
    if (p.textTransform === "uppercase") s.textTransform = "uppercase";
    if (p.italic === true) s.fontStyle = "italic";
    // `letterSpacing` is in ems, as Android's `setLetterSpacing` takes it.
    if (num(p.letterSpacing)) s.letterSpacing = `${p.letterSpacing}em`;
    // The reply preview's text starts after the avatar and name drawn over
    // its first line.
    if (num(p.firstLineIndent)) s.textIndent = `${p.firstLineIndent}px`;
    if (p.textAlign === "center") s.textAlign = "center";
    else if (p.textAlign === "right" || p.textAlign === "end") s.textAlign = "right";
    s.whiteSpace = "pre-wrap";
    s.overflowWrap = "anywhere";
    const lines = num(p.maxLines);
    if (lines != null && lines <= 1) Object.assign(s, { whiteSpace: "pre", overflow: "hidden", textOverflow: "ellipsis" });
    else if (lines != null) Object.assign(s, { display: "-webkit-box", webkitBoxOrient: "vertical", webkitLineClamp: `${lines}`, overflow: "hidden" });
    if (Array.isArray(p.spans)) {
      for (const sp of p.spans) {
        if (!sp || typeof sp !== "object") continue;
        if (sp.image && typeof sp.image === "object") {
          const size = num(sp.image.size) ?? 20;
          const img = this.drawableEl(str(sp.image.uri) || "", sp.image.tint, size, size);
          img.style.display = "inline-block";
          img.style.verticalAlign = "middle";
          if (num(sp.image.radius)) img.style.borderRadius = `${sp.image.radius}px`;
          el.append(img);
          continue;
        }
        const span = document.createElement("span");
        span.textContent = str(sp.text) || "";
        const ss = span.style;
        if (css(sp.color)) ss.color = css(sp.color);
        if (css(sp.bg)) ss.backgroundColor = css(sp.bg);
        if (sp.bold === true) ss.fontWeight = "700";
        else if (num(sp.weight) > 0) ss.fontWeight = `${sp.weight}`;
        if (sp.italic === true) ss.fontStyle = "italic";
        const deco = [sp.underline === true && "underline", sp.strike === true && "line-through"].filter(Boolean);
        if (deco.length) ss.textDecoration = deco.join(" ");
        if (sp.code === true) ss.fontFamily = "var(--font-mono)";
        el.append(span);
      }
    } else {
      el.textContent = str(p.text) || "";
    }
  }

  input(n, el) {
    const p = n.p;
    const s = el.style;
    Object.assign(s, { display: "flex", flexDirection: "row", alignItems: "center", fontFamily: "var(--font-sans)" });
    s.fontSize = `${num(p.fontSize) ?? 16}px`;
    const value = str(p.value);
    const span = document.createElement("span");
    if (value) {
      span.textContent = p.secure === true ? "•".repeat(value.length) : value;
      if (css(p.color)) span.style.color = css(p.color);
    } else {
      span.textContent = str(p.placeholder) || "";
      if (css(p.placeholderColor)) span.style.color = css(p.placeholderColor);
    }
    span.style.whiteSpace = "pre";
    span.style.overflow = "hidden";
    el.append(span);
  }

  image(n, el, rest) {
    const p = n.p;
    const s = el.style;
    s.display = "flex";
    s.alignItems = "center";
    s.justifyContent = "center";
    const uri = str(p.uri) || "";
    const tint = rest.tint ?? p.tint;
    const name = uri.startsWith("res://") ? uri.slice(6) : null;
    const d = name && this.drawables[name];
    if (d && num(p.width) == null && num(p.height) == null && d.width && d.height) {
      s.width = `${d.width}px`;
      s.height = `${d.height}px`;
    }
    const fit = { fit: "contain", stretch: "fill" }[p.scale] || "cover";
    let inner;
    if (name) inner = this.drawableEl(uri, tint, null, null, fit);
    else {
      // A remote image (an avatar, an attachment): the fixtures carry none,
      // so the placeholder the client shows until it loads.
      const ph = str(p.placeholder);
      inner = ph && ph.startsWith("res://") && this.drawables[ph.slice(6)]
        ? this.drawableEl(ph, null, null, null, "cover")
        : Object.assign(document.createElement("div"), { className: "remote" });
    }
    inner.style.width = "100%";
    inner.style.height = "100%";
    const r = num(p.radius);
    if (r != null) inner.style.borderRadius = `${r}px`;
    el.append(inner);
  }

  // A drawable, tinted with `tint`: the user's own if they loaded it, else
  // a box in its tint.
  drawableEl(uri, tint, w, h, fit = "contain") {
    const name = uri.startsWith("res://") ? uri.slice(6) : uri;
    const d = this.drawables[name];
    let el;
    if (d && css(tint)) {
      el = document.createElement("div");
      el.style.backgroundColor = css(tint);
      const m = `url("${d.uri}") center / ${fit === "fill" ? "100% 100%" : fit} no-repeat`;
      el.style.mask = m;
      el.style.webkitMask = m;
    } else if (d) {
      el = document.createElement("img");
      el.src = d.uri;
      el.style.objectFit = fit;
    } else {
      this.missing.add(name);
      el = document.createElement("div");
      el.className = "nodrawable";
      el.title = name;
      if (css(tint)) el.style.setProperty("--tint", css(tint));
    }
    if (w != null) el.style.width = `${w}px`;
    if (h != null) el.style.height = `${h}px`;
    return el;
  }

  // A popover, over the whole screen, by its anchor (its parent).
  popover(id, host) {
    const p = this.get(id).p;
    const wrap = document.createElement("div");
    wrap.style.cssText = "position:absolute;inset:0";
    if (p.passthrough !== true && css(p.scrim)) wrap.style.background = css(p.scrim);
    this.hoisting = true;
    const box = document.createElement("div");
    box.style.position = "absolute";
    box.append(this.node(id, LAYER));
    wrap.append(box);
    host.append(wrap);
    const anchorId = Object.keys(this.tree.nodes).find((k) => this.tree.nodes[k].c.includes(+id));
    const anchor = anchorId && this.els.get(+anchorId);
    if (!anchor) return;
    const hr = host.getBoundingClientRect(), a = anchor.getBoundingClientRect(), b = box.getBoundingClientRect();
    const offset = num(p.offset) ?? 0, inset = num(p.edgeInset) ?? 12;
    const [left, top, right, bottom] = [a.left - hr.left, a.top - hr.top, a.right - hr.left, a.bottom - hr.top];
    const y = p.placement === "top" ? top - offset - b.height : bottom + offset;
    const align = str(p.align) || "center";
    const x = align === "start" ? left : align === "end" ? right - b.width : Math.min(Math.max((left + right - b.width) / 2, inset), hr.width - inset - b.width);
    box.style.left = `${x}px`;
    box.style.top = `${y}px`;
  }
}

// A stack's layer above the first: over the whole stack, placed by its
// `gravity` ("top|start" when it has none).
function layer(child, gravity) {
  const parts = new Set((gravity || "top|start").split("|"));
  const w = document.createElement("div");
  w.className = "layer";
  w.style.justifyContent = parts.has("bottom") ? "flex-end" : parts.has("center") || parts.has("center_vertical") ? "center" : "flex-start";
  w.style.alignItems = parts.has("end") || parts.has("right") ? "flex-end" : parts.has("center") || parts.has("center_horizontal") ? "center" : "flex-start";
  w.append(child);
  return w;
}

// A `backgroundGradient`: a record of `angle` (180 from top to bottom),
// `colors` and `positions`, or a bare array along `backgroundGradientAxis`.
function gradient(v, axis) {
  let angle, colors, positions = [];
  if (Array.isArray(v)) {
    angle = axis === "vertical" ? 180 : 90;
    colors = v;
  } else if (v && typeof v === "object") {
    angle = num(v.angle) ?? 180;
    colors = Array.isArray(v.colors) ? v.colors : [];
    positions = Array.isArray(v.positions) ? v.positions : [];
  } else return null;
  colors = colors.filter((c) => typeof c === "number");
  if (!colors.length) return null;
  const stops = colors.map((c, i) => {
    const at = num(positions[i]) ?? (colors.length > 1 ? i / (colors.length - 1) : 0);
    return `${css(c)} ${(at * 100).toFixed(2)}%`;
  });
  return `linear-gradient(${angle}deg, ${stops.join(", ")})`;
}

function shadow(v) {
  const c = (num(v.color) ?? 0xff000000) >>> 0;
  const a = ((c >>> 24) / 255) * Math.min(1, Math.max(0, num(v.opacity) ?? 1));
  const color = `rgba(${(c >>> 16) & 255},${(c >>> 8) & 255},${c & 255},${a.toFixed(3)})`;
  return `${num(v.offsetX) ?? 0}px ${num(v.offsetY) ?? 0}px ${num(v.radius) ?? 0}px ${color}`;
}

