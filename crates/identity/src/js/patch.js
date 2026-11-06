// Fingerprint patches for values CDP Emulation cannot set. Runs before any page script
// (Page.addScriptToEvaluateOnNewDocument), once per document — or, for workers, by
// Runtime.evaluate while the worker is still paused (see worker.rs). `cfg` values that are
// null are left alone.
//
// Rules that keep the patches hard to notice:
// * replace getters on the *prototype* (Navigator.prototype / WorkerNavigator.prototype /
//   Screen.prototype), never add own properties to `navigator`
//   (Object.getOwnPropertyNames(navigator) stays empty);
// * replacements are method-shorthand functions: no `prototype`, same `name` and `length` as
//   the native ones, and they call the native getter first so a wrong `this` still throws
//   "Illegal invocation";
// * Function.prototype.toString is wrapped so the replacements print "[native code]" — and
//   the wrapper itself prints like the native toString.
// * no globals are left behind; everything lives in this closure.
(() => {
  const cfg = __CONFIG__;
  const nativeToString = Function.prototype.toString;
  const fakes = new WeakMap();
  const toString = new Proxy(nativeToString, {
    apply(target, self, args) {
      if (fakes.has(self)) return fakes.get(self);
      return Reflect.apply(target, self, args);
    },
  });
  fakes.set(toString, Reflect.apply(nativeToString, nativeToString, []));
  Object.defineProperty(Function.prototype, 'toString', {
    value: toString, writable: true, enumerable: false, configurable: true,
  });
  const nativeLike = (fn, name) => fakes.set(fn, `function ${name}() { [native code] }`);

  // Replace the getter `prop` of `proto` so it returns `value`.
  const getter = (proto, prop, value) => {
    const desc = proto && Object.getOwnPropertyDescriptor(proto, prop);
    if (!desc || typeof desc.get !== 'function') return;
    const native = desc.get;
    const holder = { get [prop]() { Reflect.apply(native, this, []); return value; } };
    const fn = Object.getOwnPropertyDescriptor(holder, prop).get;
    nativeLike(fn, `get ${prop}`);
    Object.defineProperty(proto, prop, { ...desc, get: fn });
  };
  const given = (v) => v !== null && v !== undefined;

  // Pages have Navigator, workers WorkerNavigator.
  const nav = typeof Navigator !== 'undefined' ? Navigator.prototype
    : typeof WorkerNavigator !== 'undefined' ? WorkerNavigator.prototype : null;
  if (given(cfg.hardwareConcurrency)) getter(nav, 'hardwareConcurrency', cfg.hardwareConcurrency);
  if (given(cfg.deviceMemory)) getter(nav, 'deviceMemory', cfg.deviceMemory);
  // Automation flag: a normal user's browser says false (workers have no webdriver).
  getter(nav, 'webdriver', false);
  // Worker fallbacks (pages get these from Network.setUserAgentOverride).
  if (given(cfg.userAgent)) getter(nav, 'userAgent', cfg.userAgent);
  if (given(cfg.appVersion)) getter(nav, 'appVersion', cfg.appVersion);
  if (given(cfg.platform)) getter(nav, 'platform', cfg.platform);
  if (given(cfg.languages) && cfg.languages.length) {
    // navigator.languages is a frozen array, the same object on every read.
    const languages = Object.freeze(cfg.languages.slice());
    getter(nav, 'language', languages[0]);
    getter(nav, 'languages', languages);
  }

  // Screen: under emulation avail* equals the screen size; a real desktop has a taskbar.
  const scr = typeof Screen !== 'undefined' ? Screen.prototype : null;
  if (given(cfg.availWidth)) getter(scr, 'availWidth', cfg.availWidth);
  if (given(cfg.availHeight)) getter(scr, 'availHeight', cfg.availHeight);

  // WebGL: the unmasked GPU strings (WEBGL_debug_renderer_info), WebGL 1 and 2, also for
  // OffscreenCanvas in workers.
  if (cfg.webglVendor || cfg.webglRenderer) {
    const UNMASKED_VENDOR = 0x9245;
    const UNMASKED_RENDERER = 0x9246;
    for (const name of ['WebGLRenderingContext', 'WebGL2RenderingContext']) {
      const proto = self[name] && self[name].prototype;
      if (!proto) continue;
      const native = proto.getParameter;
      const holder = {
        getParameter(pname) {
          const real = Reflect.apply(native, this, arguments);
          if (pname === UNMASKED_VENDOR && cfg.webglVendor) return cfg.webglVendor;
          if (pname === UNMASKED_RENDERER && cfg.webglRenderer) return cfg.webglRenderer;
          return real;
        },
      };
      nativeLike(holder.getParameter, 'getParameter');
      Object.defineProperty(proto, 'getParameter', {
        ...Object.getOwnPropertyDescriptor(proto, 'getParameter'),
        value: holder.getParameter,
      });
    }
  }
})();
