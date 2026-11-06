// BrowserMCP link hints: the page half (the Rust half is crates/hints/src/service.rs).
//
// Runs in the isolated world "bmcp-hints": page scripts cannot see these variables, and the
// labels live in a closed shadow root, so page CSS and scripts cannot touch them either.
// Rust owns the logic (which labels, what typing means, the action). This script only
//   * finds the interactive elements in the viewport            -> collect()
//   * draws the labels and keeps them in place on scroll/resize -> render() / filter() / hide()
//   * reports keys while hints are up, and the trigger keys (f, F…) while they are not.
// Events go to Rust through the CDP binding __bmcpHintsEvent; until Rust has added it to this
// document (right after a navigation) they wait in a queue that Rust drains.
(() => {
  if (window.top !== window) return; // hints are for the top document only
  if (globalThis.__bmcpHints) return; // already running in this document

  const BINDING = "__bmcpHintsEvent";
  /** {enabled, keys: {click: "f", newtab: "F", …}, disabledSites: ["github.com"]} */
  let config = __CONFIG__;
  const queue = [];

  function send(event) {
    const text = JSON.stringify(event);
    const binding = globalThis[BINDING];
    if (typeof binding === "function") binding(text);
    else if (queue.length < 50) queue.push(text);
  }

  // ---------------------------------------------------------------- finding elements

  // The same idea of "interactive" as the snapshot's refs (crates/perception/src/roles.rs):
  // native controls, ARIA widget roles, and focusable / clickable custom elements.
  const SELECTOR = [
    "a[href]", "area[href]", "button", "input:not([type=hidden])", "select", "textarea",
    "summary", "[contenteditable='']", "[contenteditable=true]", "[onclick]",
    "[tabindex]:not([tabindex='-1'])",
    ...["button", "link", "checkbox", "radio", "switch", "tab", "menuitem", "menuitemcheckbox",
      "menuitemradio", "option", "treeitem", "combobox", "textbox", "searchbox", "slider",
      "spinbutton"].map((r) => `[role=${r}]`),
  ].join(",");
  /** An element inside one of these that already has a label gets none of its own. */
  const OUTER = "a[href],button,[role=button],[role=link]";
  const MAX = 2000;

  /** The document and every open shadow root in it. */
  function roots() {
    const out = [document];
    for (let i = 0; i < out.length; i++) {
      for (const el of out[i].querySelectorAll("*")) if (el.shadowRoot) out.push(el.shadowRoot);
    }
    return out;
  }

  /** The first box of `el` that is at least 3x3 px inside the viewport, clipped to it. */
  function visibleRect(el, vw, vh) {
    for (const r of el.getClientRects()) {
      const left = Math.max(r.left, 0), top = Math.max(r.top, 0);
      const right = Math.min(r.right, vw), bottom = Math.min(r.bottom, vh);
      if (right - left >= 3 && bottom - top >= 3) return { left, top, right, bottom };
    }
    return null;
  }

  /** A box for `el`, or for its first child with one (an inline <a> around a block <img>). */
  function boxOf(el, vw, vh) {
    const r = visibleRect(el, vw, vh);
    if (r) return r;
    for (let c = el.firstElementChild, n = 0; c && n < 3; c = c.nextElementSibling, n++) {
      const cr = visibleRect(c, vw, vh);
      if (cr) return cr;
    }
    return null;
  }

  /** Would a click on the box reach `el` (nothing like a sticky header or a dialog on top)? */
  function reachable(el, r) {
    const root = el.getRootNode();
    const probe = typeof root.elementFromPoint === "function" ? root : document;
    const points = [
      [(r.left + r.right) / 2, (r.top + r.bottom) / 2],
      [r.left + 1, r.top + 1],
      [r.right - 1, r.bottom - 1],
    ];
    for (const [x, y] of points) {
      const hit = probe.elementFromPoint(x, y);
      if (hit && (hit === el || el.contains(hit) || hit.control === el)) return true;
    }
    return false;
  }

  function disabled(el) {
    return el.disabled === true || el.getAttribute("aria-disabled") === "true";
  }

  function shown(el) {
    return typeof el.checkVisibility !== "function" ||
      el.checkVisibility({ checkOpacity: true, checkVisibilityCSS: true });
  }

  /** Short facts for Rust (never an input's value: it may be a password). */
  function describe(el) {
    const tag = el.tagName.toLowerCase();
    const field = tag === "input" || tag === "textarea" || tag === "select";
    const img = el.querySelector && el.querySelector("img[alt]");
    const text = (el.getAttribute("aria-label") ||
      (field ? el.placeholder || el.name || el.type || "" : el.textContent) ||
      el.title || (img && img.alt) || "").replace(/\s+/g, " ").trim().slice(0, 60);
    // `href` of an SVG <a> is an object, not a string.
    const href = typeof el.href === "string" && el.href ? el.href : null;
    return { tag, text, href };
  }

  // ---------------------------------------------------------------- drawing

  const CSS = `
    :host { all: initial; }
    .h {
      position: absolute; left: 0; top: 0; box-sizing: border-box;
      padding: 0 3px; min-width: 15px; text-align: center; white-space: nowrap;
      font: 700 11px/15px ui-monospace, "Cascadia Mono", Consolas, Menlo, monospace;
      letter-spacing: .04em; text-transform: uppercase;
      color: #1d1d1d; background: linear-gradient(#fff4a8, #ffd84a);
      border: 1px solid #c79f1c; border-radius: 3px;
      box-shadow: 0 1px 3px rgba(0, 0, 0, .4);
      pointer-events: none; will-change: transform;
    }
    .h .t { color: #9c7c00; }
  `;
  // The host is a fixed 0x0 box (no layout shift, no hit area); inline !important beats
  // page CSS. As a manual popover it sits in the top layer, above modal dialogs too.
  const HOST_STYLE = "all:initial!important;display:block!important;position:fixed!important;" +
    "inset:0 auto auto 0!important;width:0!important;height:0!important;margin:0!important;" +
    "padding:0!important;border:0!important;background:transparent!important;" +
    "overflow:visible!important;z-index:2147483647!important;pointer-events:none!important;";

  let state = "idle"; // idle | armed (trigger key sent, waiting for render) | active
  let armTimer = 0;
  let items = []; // elements from the last collect(), by index
  let labels = []; // labels on screen, parallel to items
  let marks = []; // label <span>s
  let match = []; // label i starts with what was typed
  let host = null;
  let raf = 0;

  function mount() {
    host = document.createElement("bmcp-hints");
    host.setAttribute("style", HOST_STYLE);
    host.setAttribute("popover", "manual");
    const shadow = host.attachShadow({ mode: "closed" });
    const style = document.createElement("style");
    style.textContent = CSS;
    shadow.appendChild(style);
    (document.documentElement || document.body).appendChild(host);
    try { host.showPopover(); } catch (e) { /* no popover support: z-index still applies */ }
    return shadow;
  }

  /** Put every label at the top-left corner of its element (hidden when it left the view). */
  function place() {
    raf = 0;
    const vw = window.innerWidth, vh = window.innerHeight;
    const dpr = window.devicePixelRatio || 1;
    const snap = (v) => Math.round(v * dpr) / dpr; // whole device pixels: crisp text
    for (let i = 0; i < marks.length; i++) {
      const el = items[i];
      const r = match[i] && el.isConnected ? boxOf(el, vw, vh) : null;
      const m = marks[i];
      if (!r) {
        m.style.display = "none";
        continue;
      }
      m.style.display = "";
      const x = Math.min(Math.max(r.left - 2, 0), Math.max(vw - 24, 0));
      const y = Math.min(Math.max(r.top - 2, 0), Math.max(vh - 16, 0));
      m.style.transform = `translate(${snap(x)}px, ${snap(y)}px)`;
    }
  }

  function schedule() {
    if (!raf) raf = requestAnimationFrame(place);
  }

  /** Hints are up: someone clicked the page instead of typing. */
  function onMouseDown(e) {
    if (!e.isTrusted) return;
    hide();
    send({ type: "cancel" });
  }

  function listen(on) {
    const method = on ? "addEventListener" : "removeEventListener";
    window[method]("scroll", schedule, { capture: true, passive: true });
    window[method]("resize", schedule, { passive: true });
    window[method]("mousedown", onMouseDown, true);
  }

  function hide() {
    if (host) host.remove();
    if (state === "active") listen(false);
    host = null;
    labels = [];
    marks = [];
    match = [];
    state = "idle";
  }

  // ---------------------------------------------------------------- keys

  const held = new Set(); // codes whose keydown we swallowed: swallow keypress/keyup too

  function block(e) {
    e.preventDefault();
    e.stopImmediatePropagation();
    held.add(e.code || e.key);
  }

  /** The focused element, looking into shadow roots. */
  function focused() {
    let a = document.activeElement;
    while (a && a.shadowRoot && a.shadowRoot.activeElement) a = a.shadowRoot.activeElement;
    return a;
  }

  const NOT_TEXT = ["button", "submit", "reset", "checkbox", "radio", "image", "file", "range", "color"];

  function editable(el) {
    if (document.designMode === "on") return true;
    if (!el || el === document.body || el === document.documentElement) return false;
    if (el.isContentEditable) return true;
    const tag = el.tagName;
    if (tag === "TEXTAREA" || tag === "SELECT" || tag === "IFRAME") return true;
    if (tag === "INPUT") return !NOT_TEXT.includes((el.type || "").toLowerCase());
    const role = el.getAttribute("role");
    return role === "textbox" || role === "searchbox" || role === "combobox";
  }

  function siteOff() {
    const host = location.hostname.replace(/^www\./, "");
    return (config.disabledSites || []).some((s) => host === s || host.endsWith("." + s));
  }

  function onKeyDown(e) {
    // Page scripts cannot drive hints (synthetic events), and shortcuts pass through.
    if (!e.isTrusted || e.ctrlKey || e.altKey || e.metaKey) return;
    if (state !== "idle") {
      if (e.key === "Shift" || e.key === "CapsLock") return;
      block(e);
      if (!e.repeat) send({ type: "key", key: e.key });
      return;
    }
    if (!config.enabled || e.repeat || e.isComposing || editable(focused()) || siteOff()) return;
    const keys = config.keys || {};
    const mode = Object.keys(keys).find((m) => keys[m] === e.key);
    if (!mode) return;
    block(e);
    // Keys typed before the labels appear are reported too; Rust handles them after showing.
    state = "armed";
    clearTimeout(armTimer);
    armTimer = setTimeout(() => { if (state === "armed") state = "idle"; }, 3000);
    send({ type: "trigger", mode });
  }

  function onKeyOther(e) {
    const id = e.code || e.key;
    if (!held.has(id)) return;
    e.preventDefault();
    e.stopImmediatePropagation();
    if (e.type === "keyup") held.delete(id);
  }

  window.addEventListener("keydown", onKeyDown, true);
  window.addEventListener("keypress", onKeyOther, true);
  window.addEventListener("keyup", onKeyOther, true);
  window.addEventListener("pagehide", hide);

  // ---------------------------------------------------------------- API for Rust

  globalThis.__bmcpHints = {
    /** Find the elements to label (hides old hints). Returns {items: [{tag, text, href}], ms}. */
    collect() {
      const t0 = performance.now();
      if (state === "active") hide();
      const vw = window.innerWidth, vh = window.innerHeight;
      const found = [];
      const chosen = new Set();
      for (const root of roots()) {
        for (const el of root.querySelectorAll(SELECTOR)) {
          if (found.length >= MAX) break;
          if (chosen.has(el) || disabled(el)) continue;
          let target = el;
          let r = boxOf(el, vw, vh);
          if (r && !shown(el)) r = null;
          // A hidden checkbox styled through its <label>: label the label.
          if (!r && el.labels && el.labels.length) {
            target = el.labels[0];
            r = boxOf(target, vw, vh);
            if (chosen.has(target)) continue;
          }
          if (!r) continue;
          const outer = el.parentElement && el.parentElement.closest(OUTER);
          if (outer && chosen.has(outer)) continue;
          if (!reachable(target, r)) continue;
          chosen.add(el);
          chosen.add(target);
          found.push(target);
        }
      }
      items = found;
      return { items: found.map(describe), ms: performance.now() - t0 };
    },

    /** Show `labels[i]` next to element i of the last collect(). Returns how many. */
    render(newLabels) {
      if (state === "active") hide();
      clearTimeout(armTimer);
      if (!newLabels.length) {
        state = "idle";
        return 0;
      }
      const shadow = mount();
      labels = newLabels.slice(0, items.length);
      match = labels.map(() => true);
      const frag = document.createDocumentFragment();
      marks = labels.map((label) => {
        const m = document.createElement("span");
        m.className = "h";
        m.textContent = label;
        frag.appendChild(m);
        return m;
      });
      shadow.appendChild(frag);
      place();
      state = "active";
      listen(true);
      return labels.length;
    },

    /** Only labels starting with `typed` stay; the typed part is dimmed. */
    filter(typed) {
      for (let i = 0; i < marks.length; i++) {
        const label = labels[i];
        match[i] = label.startsWith(typed);
        if (!match[i]) continue;
        const done = document.createElement("span");
        done.className = "t";
        done.textContent = typed;
        marks[i].replaceChildren(done, label.slice(typed.length));
      }
      place();
      return match.filter(Boolean).length;
    },

    hide,
    /** Element i of the last collect() (still there after hide(), for the action). */
    element: (i) => items[i] || null,
    configure(c) { config = c; },
    drain: () => queue.splice(0),
    get state() { return state; },

    /** Copy text: the async clipboard API, else execCommand (needs a user gesture). */
    async copy(text) {
      try {
        await navigator.clipboard.writeText(text);
        return "clipboard_api";
      } catch (e) { /* not focused, or no permission: try the old way */ }
      const area = document.createElement("textarea");
      area.value = text;
      area.setAttribute("style", "position:fixed!important;left:-9999px!important;top:0!important;opacity:0!important;");
      const before = document.activeElement;
      (document.body || document.documentElement).appendChild(area);
      area.select();
      let ok = false;
      try { ok = document.execCommand("copy"); } catch (e) { ok = false; }
      area.remove();
      if (before && typeof before.focus === "function") before.focus({ preventScroll: true });
      return ok ? "exec_command" : "failed";
    },
  };
})();
