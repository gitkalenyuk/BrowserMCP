// stealth.check: probes a fingerprinting script would run, evaluated in the page's main world.
// `expected` is the identity applied to the tab (or null). Returns
// { checks: [{ name, status: "pass"|"warn"|"fail", detail }] }.
(async (expected) => {
  const checks = [];
  const add = (name, status, detail) => checks.push({ name, status, detail: String(detail) });
  const ok = (name, cond, detail, bad = 'fail') => add(name, cond ? 'pass' : bad, detail);
  const safe = (fn, fallback) => { try { return fn(); } catch (e) { return fallback; } };
  const nav = navigator;
  const ua = nav.userAgent;

  // 1. Automation flags.
  ok('navigator.webdriver', nav.webdriver !== true, `webdriver=${nav.webdriver}`);
  const own = Object.getOwnPropertyNames(nav);
  ok('navigator own properties', own.length === 0,
     own.length ? `own: ${own.join(', ')} (patched on the instance)` : 'none (clean)', 'warn');
  ok('headless UA', !/HeadlessChrome/.test(ua), ua.slice(0, 120));
  const cdcKeys = Object.keys(window).filter((k) => /^(cdc_|\$cdc_|__webdriver|__selenium|__playwright|__pw|__puppeteer)/.test(k));
  ok('automation globals', cdcKeys.length === 0, cdcKeys.length ? cdcKeys.join(', ') : 'none');

  // 2. UA ↔ client hints ↔ navigator.platform.
  const major = (ua.match(/Chrome\/(\d+)/) || [])[1];
  const uad = nav.userAgentData;
  if (major && uad) {
    const brand = uad.brands.find((b) => b.brand === 'Google Chrome' || b.brand === 'Chromium');
    ok('UA vs UA-CH version', brand && brand.version === major,
       `UA Chrome/${major}, brands ${uad.brands.map((b) => b.brand + '/' + b.version).join(', ')}`);
    const hasChrome = uad.brands.some((b) => b.brand === 'Google Chrome');
    ok('UA-CH brand Google Chrome', hasChrome,
       hasChrome ? 'present' : 'missing (looks like plain Chromium/CEF)', 'warn');
    const uaPlat = /Windows/.test(ua) ? 'Windows' : /Android/.test(ua) ? 'Android'
      : /Mac OS X/.test(ua) ? 'macOS' : /Linux/.test(ua) ? 'Linux' : '?';
    ok('UA vs UA-CH platform', uad.platform === uaPlat, `UA ${uaPlat}, UA-CH ${uad.platform}`);
    ok('UA vs UA-CH mobile', uad.mobile === /Mobile/.test(ua), `UA-CH mobile=${uad.mobile}`);
    try {
      const he = await uad.getHighEntropyValues(
        ['platformVersion', 'architecture', 'model', 'fullVersionList', 'bitness']);
      const full = (he.fullVersionList || []).find((b) => b.brand === 'Google Chrome' || b.brand === 'Chromium');
      ok('UA-CH fullVersionList', full && full.version.split('.')[0] === major,
         `${(he.fullVersionList || []).map((b) => b.brand + '/' + b.version).join(', ')}; ` +
         `platformVersion=${he.platformVersion} arch=${he.architecture} bitness=${he.bitness} model=${he.model || '-'}`);
    } catch (e) {
      add('UA-CH fullVersionList', 'warn', `getHighEntropyValues failed: ${e}`);
    }
  } else if (major) {
    add('UA vs UA-CH version', 'warn', 'navigator.userAgentData missing (insecure page or overridden without hints)');
  } else {
    add('UA vs UA-CH version', expected && expected.kind === 'device_emulation' ? 'warn' : 'fail',
        `non-Chrome UA on a Chromium engine (userAgentData ${uad ? 'present' : 'absent'})`);
  }
  const platOk = /Windows/.test(ua) ? nav.platform === 'Win32'
    : /Android/.test(ua) ? /^Linux (arm|aarch)/.test(nav.platform)
    : /iPhone/.test(ua) ? nav.platform === 'iPhone'
    : /Mac OS X/.test(ua) ? nav.platform === 'MacIntel'
    : true;
  ok('UA vs navigator.platform', platOk, `platform=${nav.platform}`);

  // 3. Timezone: Intl zone vs Date offset.
  const tz = Intl.DateTimeFormat().resolvedOptions().timeZone;
  const now = new Date();
  const local = new Date(now.toLocaleString('en-US', { timeZone: tz }));
  const utc = new Date(now.toLocaleString('en-US', { timeZone: 'UTC' }));
  const intlOffset = Math.round((utc - local) / 60000);
  const dateOffset = now.getTimezoneOffset();
  ok('timezone vs Date offset', Math.abs(intlOffset - dateOffset) <= 1,
     `${tz}: Intl offset ${intlOffset} min, Date offset ${dateOffset} min`);

  // 4. Languages: navigator.language, languages[0], Intl locale.
  const intlLocale = Intl.DateTimeFormat().resolvedOptions().locale;
  // Every entry must be a plain tag: "en;q=0.9" leaks from a badly formed override.
  const tagsOk = (nav.languages || []).every((l) => /^[A-Za-z]{2,3}(-[A-Za-z0-9]+)*$/.test(l));
  ok('languages consistent',
     nav.languages && nav.languages[0] === nav.language && tagsOk &&
       intlLocale.split('-')[0] === nav.language.split('-')[0],
     `language=${nav.language} languages=${JSON.stringify(nav.languages || [])} Intl=${intlLocale}`);

  // 5. Screen and window.
  const s = screen;
  ok('screen vs window', s.width >= innerWidth && s.height >= innerHeight && s.availWidth <= s.width,
     `screen ${s.width}x${s.height} avail ${s.availWidth}x${s.availHeight}, inner ${innerWidth}x${innerHeight}, dpr ${devicePixelRatio}`, 'warn');
  ok('outer window size', outerWidth > 0 && outerHeight > 0,
     `outer ${outerWidth}x${outerHeight} (0 = headless tell)`);

  // 6. Hardware.
  const mem = nav.deviceMemory;
  ok('deviceMemory value', mem === undefined || [0.25, 0.5, 1, 2, 4, 8, 16, 32].includes(mem),
     `deviceMemory=${mem} hardwareConcurrency=${nav.hardwareConcurrency}`);
  const touchUA = /Mobile|Android|iPhone|iPad/.test(ua);
  ok('touch vs UA', touchUA === (nav.maxTouchPoints > 0) || (!/Mobile/.test(ua) && nav.maxTouchPoints > 0 && /Macintosh/.test(ua)),
     `maxTouchPoints=${nav.maxTouchPoints} ontouchstart=${'ontouchstart' in window}`, 'warn');

  // 7. WebGL strings.
  const gl = safe(() => document.createElement('canvas').getContext('webgl'), null);
  let vendor = '', renderer = '';
  if (gl) {
    const ext = gl.getExtension('WEBGL_debug_renderer_info');
    if (ext) {
      vendor = gl.getParameter(ext.UNMASKED_VENDOR_WEBGL);
      renderer = gl.getParameter(ext.UNMASKED_RENDERER_WEBGL);
    }
    const sw = /SwiftShader|llvmpipe|Software/i.test(renderer);
    const osOk = /Windows/.test(ua) ? /Direct3D|D3D/.test(renderer)
      : /Mac OS X/.test(ua) && !/Mobile/.test(ua) ? /Metal|OpenGL|Apple/.test(renderer) : true;
    ok('WebGL renderer', !sw && osOk, `${vendor} | ${renderer}`, sw ? 'fail' : 'warn');
  } else {
    add('WebGL renderer', 'warn', 'WebGL unavailable');
  }

  // 8. window.chrome (present in real Chrome, desktop and Android).
  const chromeObj = window.chrome;
  ok('window.chrome', typeof chromeObj === 'object' && chromeObj !== null,
     chromeObj ? `keys: ${Object.keys(chromeObj).join(', ') || '(none)'}` : 'missing',
     /Chrome/.test(ua) ? 'fail' : 'pass');

  // 9. Permissions: Notification.permission must agree with the Permissions API.
  try {
    const st = (await nav.permissions.query({ name: 'notifications' })).state;
    const np = typeof Notification !== 'undefined' ? Notification.permission : 'n/a';
    const agree = np === 'n/a' || (np === 'default' ? st === 'prompt' : np === st);
    ok('permissions consistency', agree, `Notification.permission=${np}, permissions.query=${st}`);
  } catch (e) {
    add('permissions consistency', 'warn', `permissions.query failed: ${e}`);
  }

  // 10. Plugins / PDF viewer (desktop Chrome lists 5 PDF plugins).
  ok('plugins', touchUA || nav.plugins.length > 0,
     `plugins=${nav.plugins.length} pdfViewerEnabled=${nav.pdfViewerEnabled}`, 'warn');

  // 11. DevTools protocol detection: with Runtime domain enabled, console messages are
  // serialized for the client and the getter below runs.
  let stackRead = false;
  const err = new Error('probe');
  Object.defineProperty(err, 'stack', { configurable: true, get() { stackRead = true; return ''; } });
  console.debug(err);
  let idRead = false;
  const div = document.createElement('div');
  Object.defineProperty(div, 'id', { get() { idRead = true; return 'x'; } });
  console.debug(div);
  await new Promise((r) => setTimeout(r, 30));
  ok('CDP console serialization', !stackRead && !idRead,
     stackRead || idRead ? 'console arguments were inspected (Runtime domain enabled)' : 'not observed');
  ok('Error.prepareStackTrace', typeof Error.prepareStackTrace === 'undefined',
     `typeof=${typeof Error.prepareStackTrace}`, 'warn');

  // 12. Patched functions must look native.
  const native = (f) => typeof f === 'function' && /\{\s*\[native code\]\s*\}$/.test(Function.prototype.toString.call(f));
  const getters = ['hardwareConcurrency', 'deviceMemory', 'webdriver']
    .map((p) => Object.getOwnPropertyDescriptor(Navigator.prototype, p))
    .filter((d) => d && d.get).map((d) => d.get);
  const glGet = typeof WebGLRenderingContext !== 'undefined' ? WebGLRenderingContext.prototype.getParameter : null;
  const fns = [...getters, Function.prototype.toString, ...(glGet ? [glGet] : [])];
  const nonNative = fns.filter((f) => !native(f));
  ok('toString of patched functions', nonNative.length === 0,
     nonNative.length ? `${nonNative.length} function(s) print their source` : `${fns.length} checked, all native-looking`);
  ok('patched getters have no prototype', getters.every((g) => !('prototype' in g)),
     'getter functions are prototype-less like native ones');

  // 13. Workers see the same navigator (CreepJS compares them).
  try {
    const src = 'postMessage([navigator.userAgent, navigator.hardwareConcurrency, navigator.language, Intl.DateTimeFormat().resolvedOptions().timeZone])';
    const url = URL.createObjectURL(new Blob([src], { type: 'application/javascript' }));
    const w = new Worker(url);
    const data = await new Promise((res, rej) => {
      w.onmessage = (e) => res(e.data); w.onerror = (e) => rej(e.message || 'worker error');
      setTimeout(() => rej('timeout'), 2000);
    });
    w.terminate(); URL.revokeObjectURL(url);
    const same = data[0] === ua && data[1] === nav.hardwareConcurrency && data[2] === nav.language && data[3] === tz;
    ok('worker navigator', same,
       same ? 'UA, cores, language and timezone match' :
       `worker: cores=${data[1]} lang=${data[2]} tz=${data[3]} ua ${data[0] === ua ? 'same' : 'differs'}`, 'warn');
  } catch (e) {
    add('worker navigator', 'warn', `worker probe failed: ${e}`);
  }

  // 14. Expected identity (what stealth.fingerprint_set applied).
  if (expected) {
    const diffs = [];
    const cmp = (what, got, want) => { if (want !== undefined && want !== null && got !== want) diffs.push(`${what}: ${got} != ${want}`); };
    cmp('userAgent', ua, expected.user_agent);
    cmp('platform', nav.platform, expected.platform);
    cmp('hardwareConcurrency', nav.hardwareConcurrency, expected.hardware_concurrency);
    if (mem !== undefined) cmp('deviceMemory', mem, expected.device_memory);
    cmp('timezone', tz, expected.timezone);
    cmp('language', nav.language, expected.locale);
    cmp('screen.width', s.width, expected.screen && expected.screen.width);
    cmp('screen.height', s.height, expected.screen && expected.screen.height);
    cmp('devicePixelRatio', devicePixelRatio, expected.screen && expected.screen.dpr);
    cmp('maxTouchPoints', nav.maxTouchPoints, expected.max_touch_points);
    if (gl) { cmp('webgl vendor', vendor, expected.webgl_vendor); cmp('webgl renderer', renderer, expected.webgl_renderer); }
    ok('matches applied identity', diffs.length === 0,
       diffs.length ? diffs.join('; ') : `identity ${expected.id} fully applied`);
  }
  return { checks };
})(__EXPECTED__)
