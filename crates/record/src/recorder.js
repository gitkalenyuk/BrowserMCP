// BrowserMCP action recorder.
//
// Injected by bmcp-record into the isolated world "bmcp-recorder": it shares the DOM with the
// page but not the JavaScript globals, so page scripts cannot see or break it. It reports what
// a person does (clicks, typing, keys, selects, checkboxes, scrolling) as JSON strings:
//
// * MODE "binding": through the CDP binding `__bmcpRecord` (Runtime.addBinding) -> the browser
//   gets a Runtime.bindingCalled event at once;
// * MODE "queue":   into a queue the browser drains every 250 ms (`__bmcpRecorder.drain()`).
//
// Only trusted events (real input, or CDP Input.* which the page cannot tell apart) count.
// The Rust side replaces __BMCP_MODE__ before injecting.
(() => {
  if (globalThis.__bmcpRecorder) return;

  const MAX_TEXT = 80;
  const queue = [];
  const listeners = [];
  // Text fields being typed into: element -> time of the last input event.
  const pending = new Map();
  let mode = "__BMCP_MODE__";
  let scrollTimer = 0;

  // ---------------------------------------------------------------- sending

  function send(event) {
    if (event.t === undefined) event.t = Date.now();
    event.url = location.href;
    const line = JSON.stringify(event);
    if (mode === "binding" && typeof globalThis.__bmcpRecord === "function") {
      try {
        globalThis.__bmcpRecord(line);
        return;
      } catch (_) {
        // fall through to the queue
      }
    }
    queue.push(line);
    if (queue.length > 5000) queue.shift();
  }

  // ---------------------------------------------------------------- describing elements

  const clean = (s) => String(s || "").replace(/\s+/g, " ").trim().slice(0, MAX_TEXT);
  const textOf = (el) => clean(el.innerText !== undefined ? el.innerText : el.textContent);

  const WIDGET_ROLES = new Set([
    "button", "link", "checkbox", "radio", "switch", "tab", "menuitem", "menuitemcheckbox",
    "menuitemradio", "option", "combobox", "textbox", "searchbox", "slider", "spinbutton",
    "treeitem", "gridcell", "listbox",
  ]);
  // Roles whose accessible name comes from their content.
  const NAME_FROM_CONTENT = new Set([
    "button", "link", "checkbox", "radio", "switch", "tab", "menuitem", "menuitemcheckbox",
    "menuitemradio", "option", "treeitem", "heading", "cell", "gridcell", "listitem", "tooltip",
  ]);

  function inputRole(el) {
    const type = (el.getAttribute("type") || "text").toLowerCase();
    switch (type) {
      case "button": case "submit": case "reset": case "image": return "button";
      case "checkbox": return "checkbox";
      case "radio": return "radio";
      case "range": return "slider";
      case "number": return "spinbutton";
      case "hidden": return null;
      case "search": return el.hasAttribute("list") ? "combobox" : "searchbox";
      default: return el.hasAttribute("list") ? "combobox" : "textbox";
    }
  }

  // The ARIA role, as Chrome's accessibility tree would report it (simplified).
  function roleOf(el) {
    const explicit = (el.getAttribute("role") || "").trim().split(/\s+/)[0];
    if (explicit) return explicit;
    switch (el.localName) {
      case "a": case "area": return el.hasAttribute("href") ? "link" : null;
      case "button": return "button";
      case "input": return inputRole(el);
      case "textarea": return "textbox";
      case "select": return el.multiple || el.size > 1 ? "listbox" : "combobox";
      case "option": return "option";
      case "img": return "img";
      case "h1": case "h2": case "h3": case "h4": case "h5": case "h6": return "heading";
      default: return el.isContentEditable ? "textbox" : null;
    }
  }

  function labelsText(el) {
    if (!el.labels || !el.labels.length) return "";
    return clean(Array.from(el.labels).map(textOf).join(" "));
  }

  // The accessible name (the common cases of the accname spec).
  function nameOf(el) {
    const by = el.getAttribute("aria-labelledby");
    if (by) {
      const text = clean(by.split(/\s+/).map((id) => document.getElementById(id))
        .filter(Boolean).map(textOf).join(" "));
      if (text) return text;
    }
    const aria = clean(el.getAttribute("aria-label"));
    if (aria) return aria;
    const tag = el.localName;
    if (tag === "input" || tag === "textarea" || tag === "select") {
      const type = (el.getAttribute("type") || "").toLowerCase();
      if (type === "submit" || type === "reset" || type === "button") {
        return clean(el.value) || (type === "submit" ? "Submit" : type === "reset" ? "Reset" : "");
      }
      if (type === "image") return clean(el.getAttribute("alt")) || "Submit";
      return labelsText(el) || clean(el.getAttribute("placeholder")) || clean(el.getAttribute("title"));
    }
    if (tag === "img") return clean(el.getAttribute("alt")) || clean(el.getAttribute("title"));
    const role = roleOf(el);
    if (!role || NAME_FROM_CONTENT.has(role)) return textOf(el) || clean(el.getAttribute("title"));
    return clean(el.getAttribute("title"));
  }

  // The element a click on `node` was meant for: the closest control or link around it.
  function interactive(node) {
    for (let n = node; n && n.nodeType === 1; n = n.parentElement) {
      switch (n.localName) {
        case "a": case "button": case "input": case "select": case "textarea": case "summary":
        case "option":
          return n;
        case "label":
          return n.control || n;
      }
      const role = (n.getAttribute("role") || "").trim().split(/\s+/)[0];
      if (WIDGET_ROLES.has(role)) return n;
      if (n.hasAttribute("contenteditable") && n.isContentEditable) return n;
    }
    return node && node.nodeType === 1 ? node : null;
  }

  const CANDIDATES = "a,button,input,select,textarea,summary,option,[role],[contenteditable]";

  // How many earlier elements have the same role and name (0 = it is the first).
  function nthOf(el, role, name) {
    if (!role) return 0;
    let n = 0;
    for (const c of document.querySelectorAll(CANDIDATES)) {
      if (c === el) return n;
      if (roleOf(c) === role && nameOf(c) === name) n += 1;
    }
    return 0;
  }

  // A CSS selector for the element: up to the nearest id, at most 6 levels.
  function cssPath(el) {
    const parts = [];
    for (let n = el; n && n.nodeType === 1 && parts.length < 6; n = n.parentElement) {
      if (n.id && /^[A-Za-z][\w-]*$/.test(n.id)) {
        parts.unshift("#" + n.id);
        break;
      }
      let part = n.localName;
      const parent = n.parentElement;
      if (parent) {
        const same = Array.from(parent.children).filter((c) => c.localName === n.localName);
        if (same.length > 1) part += ":nth-of-type(" + (same.indexOf(n) + 1) + ")";
      }
      parts.unshift(part);
    }
    return parts.join(" > ");
  }

  function describe(el) {
    const role = roleOf(el);
    const name = nameOf(el);
    const out = { role, name: name || null, text: textOf(el) || null, css: cssPath(el), tag: el.localName };
    if (el.localName === "input") out.inputType = (el.getAttribute("type") || "text").toLowerCase();
    const nth = nthOf(el, role, name);
    if (nth > 0) out.nth = nth;
    return out;
  }

  const TEXT_TYPES = new Set(["text", "email", "password", "search", "tel", "url", "number", ""]);
  function isTextField(el) {
    if (!el || el.nodeType !== 1) return false;
    if (el.localName === "textarea") return true;
    if (el.localName === "input") return TEXT_TYPES.has((el.getAttribute("type") || "").toLowerCase());
    return el.isContentEditable;
  }
  function isToggle(el) {
    return el && el.localName === "input" && (el.type === "checkbox" || el.type === "radio");
  }
  function isSecret(el) {
    if (el.localName !== "input") return false;
    const auto = (el.getAttribute("autocomplete") || "").toLowerCase();
    return el.type === "password" || /password|one-time-code|cc-number|cc-csc/.test(auto);
  }
  const valueOf = (el) => (el.isContentEditable && el.localName !== "input" && el.localName !== "textarea"
    ? el.innerText : el.value);

  // ---------------------------------------------------------------- typing (coalesced)

  // One "fill" per field with the final value, timed at the last keystroke.
  function flush(el) {
    const t = pending.get(el);
    if (t === undefined) return;
    pending.delete(el);
    const secret = isSecret(el);
    send({ type: "fill", t, target: describe(el), value: secret ? "***" : valueOf(el), secret });
  }
  function flushAll() {
    for (const el of Array.from(pending.keys())) flush(el);
  }

  // ---------------------------------------------------------------- listeners

  function on(target, type, fn) {
    target.addEventListener(type, fn, true);
    listeners.push([target, type, fn]);
  }

  on(window, "input", (e) => {
    if (e.isTrusted && isTextField(e.target)) pending.set(e.target, Date.now());
  });

  on(window, "focusout", (e) => flush(e.target));

  on(window, "change", (e) => {
    const el = e.target;
    flush(el); // any change (also a script's) ends a typing burst
    if (!e.isTrusted) return;
    if (isToggle(el)) {
      send({ type: el.checked ? "check" : "uncheck", target: describe(el) });
    } else if (el.localName === "select") {
      const option = el.selectedOptions && el.selectedOptions[0];
      send({ type: "select", target: describe(el), value: el.value, label: option ? clean(option.text) : null });
    }
  });

  on(window, "click", (e) => {
    if (!e.isTrusted || e.detail > 1) return; // the 2nd click of a double click is the dblclick
    const el = interactive(e.target);
    if (!el) return;
    // Recorded elsewhere: checkboxes/radios and selects by "change", text fields by typing.
    if (isToggle(el) || el.localName === "select" || el.localName === "option" || isTextField(el)) return;
    flushAll();
    send({ type: "click", target: describe(el) });
  });

  on(window, "dblclick", (e) => {
    if (!e.isTrusted) return;
    const el = interactive(e.target);
    if (el) send({ type: "dblclick", target: describe(el) });
  });

  on(window, "keydown", (e) => {
    if (!e.isTrusted || !(e.key === "Enter" || e.key === "Tab" || e.key === "Escape")) return;
    const el = e.target;
    flush(el);
    const chord = (e.ctrlKey ? "Control+" : "") + (e.altKey ? "Alt+" : "") +
      (e.shiftKey ? "Shift+" : "") + (e.metaKey ? "Meta+" : "") + e.key;
    const onPage = !el || el === document.body || el === document.documentElement;
    send({ type: "key", key: chord, target: onPage ? null : describe(el) });
  });

  // Scrolling of the page, coalesced: one event 600 ms after the last scroll.
  on(window, "scroll", (e) => {
    if (e.target !== document) return;
    clearTimeout(scrollTimer);
    const t = Date.now();
    scrollTimer = setTimeout(() => send({ type: "scroll", t, x: Math.round(scrollX), y: Math.round(scrollY) }), 600);
  });

  on(window, "pagehide", () => flushAll());

  // ---------------------------------------------------------------- control from Rust

  globalThis.__bmcpRecorder = {
    get mode() { return mode; },
    set mode(value) { mode = value; },
    /** Queued events (JSON strings) since the last drain. */
    drain() {
      return queue.splice(0, queue.length);
    },
    /** Report a typing burst that has not ended yet. */
    flush: flushAll,
    stop() {
      flushAll();
      clearTimeout(scrollTimer);
      for (const [target, type, fn] of listeners) target.removeEventListener(type, fn, true);
      listeners.length = 0;
      delete globalThis.__bmcpRecorder;
    },
  };

  send({ type: "hello" });
})();
