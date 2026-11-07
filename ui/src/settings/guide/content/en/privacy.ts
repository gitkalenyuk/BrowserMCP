// Guide, English: hidden mode, blocking, performance, notifications, languages, data.
// Facts: crates/identity/README.md, docs/phase2/profiles.md, crates/adblock/README.md,
// crates/governor, crates/notify, README.md "Data folder", docs/settings/settings.md.
import { api, chapter, code, list, p, section, settings, steps, terms, warning } from '../../model';

export const stealth = chapter(
  'stealth',
  'Stealth and identity',
  'Fingerprint identities, proxies with logins and pools, timezone / language / location that fit the proxy, WebRTC protection, headless mode, device emulation and human-like input.',
  section(
    'what',
    'What it is',
    p(
      'Hidden mode makes automated browsing look like an ordinary person on an ordinary computer, and keeps several identities apart. It is built from parts you combine per **profile** (workspace): an identity (the fingerprint), a proxy (the IP address), and the language, timezone and location that fit both.',
    ),
  ),
  section(
    'why',
    'What it is for',
    list(
      'Several accounts on one site, each with its own consistent browser and IP.',
      'Checking how a site looks from another country.',
      'Automation that sites should not flag as a bot.',
      'Testing a responsive layout on phone sizes (device emulation).',
    ),
  ),
  section(
    'identity',
    'Identities (fingerprints) per profile',
    p(
      'An identity is a consistent set of browser facts: user agent and client hints, platform, screen, CPU cores, memory, WebGL vendor / renderer, timezone, locale, location. Generate one (deterministic by seed, from presets like `windows-1080p`, `macos-mbp14`, `android-pixel-8`), save it and assign it to a profile: every new tab of that profile gets it before its first page loads, including its workers.',
    ),
    steps(
      'Generate and save an identity: `identity.generate {save: true}`.',
      'Create a profile with it: `profile.create {id, identity}` (or `identity.assign`).',
      'Open tabs in that profile (workspace menu, or `tabs.open {profile}`).',
      'Check it: `stealth.check` probes the page like a fingerprinting script and returns a pass / warn / fail table.',
    ),
    // VERIFY-AFTER-MERGE: the "Profiles & identity" settings section.
    settings('profiles', 'Profiles, identities and website languages (the “Profiles & identity” section).'),
    api('identity.generate', { os: 'windows', save: true, id: 'work-id' }),
    api('profile.create', { id: 'shop', identity: 'work-id' }),
    api('stealth.check', {}),
  ),
  section(
    'proxies',
    'Proxies, logins and pools',
    list(
      'Proxy formats: `http://user:pass@host:port`, `socks5://user:pass@host:port`, `host:port` or `host:port:user:pass`.',
      'Passwords never reach the engine: every proxy is reached through a small local forwarder on 127.0.0.1. That is also how **SOCKS5 with a login** works, which Chromium cannot do by itself.',
      '**Pools**: a named list of proxies with a strategy (round robin, random, or sticky per run), a health check, and leases: one proxy per mass-run row, rotated on retry.',
      '**Split routing**: rules per profile, such as “`*.example.com` through the proxy, everything else direct” (a PAC script). There is no silent fallback to a direct connection: a dead proxy fails instead of leaking your real IP.',
    ),
    // VERIFY-AFTER-MERGE: the "Proxies" settings section.
    settings('proxies', 'Proxies and pools (the “Proxies” section).'),
    api('proxy.pool_add', { pool: 'de', proxies: ['socks5://user:pass@203.0.113.7:1080'], strategy: 'sticky' }),
    api('proxy.rules_set', { profile: 'shop', rules: [{ match: '*.example.com', via: 'pool:de' }], default: 'direct' }),
  ),
  section(
    'geo',
    'Timezone, language and location that fit the IP',
    p(
      'A German IP with a Kyiv timezone and Ukrainian browser language is an obvious tell. `identity.fit_to_proxy` looks up the proxy’s exit IP and sets the identity’s timezone, locale, languages and location (the city plus about 2 km of jitter), keeping the device itself. A mass run can do it per row with `fit_identity`.',
    ),
    api('geo.lookup', { proxy: 'socks5://user:pass@203.0.113.7:1080' }),
    api('identity.fit_to_proxy', { identity_id: 'work-id', proxy_id: 'pool:de', profile: 'shop' }),
  ),
  section(
    'webrtc',
    'WebRTC protection',
    p('When a profile uses a proxy, WebRTC is limited to the proxied route (`disable_non_proxied_udp`), so a page cannot discover your real IP through WebRTC. Without a proxy, Chromium’s defaults apply.'),
  ),
  section(
    'headless',
    'Headless mode and off-screen tabs',
    p(
      '`browsermcp.exe --headless` starts the browser without any window: every tab is an off-screen page (screenshots, video and input still work) and the process keeps running after its last tab closes, until `system.quit`. Useful on a server or for background automation. Size and pixel ratio: `--window-size 1280x800`, `--device-scale 1`.',
    ),
    code('powershell', 'browsermcp.exe --headless --window-size 1280x800\nbrowsermcp.exe mcp-stdio --headless   # an MCP client starts a headless browser'),
  ),
  section(
    'devices',
    'Device emulation and human-like input',
    list(
      '`device.emulate` turns a tab into an iPhone 15, Pixel 8, Galaxy S23 or iPad (screen, user agent, touch) for layout testing. It is **not** stealth: it keeps the real timezone and language.',
      '`input.humanlike` switches a tab to human-like input: curved mouse paths with easing, typing character by character with natural delays, wheel scrolling. Slower, but looks like a person.',
    ),
    api('device.emulate', { device: 'iPhone 15' }),
    api('input.humanlike', { on: true }),
  ),
  section(
    'limits',
    'Limits and known issues',
    list(
      'Fonts are the ones installed on your computer; `stealth.check` reports when they do not fit the claimed OS.',
      'The claimed browser is always Chrome of the engine’s version (other versions would be revealed by feature detection); iPhone / iPad presets are for layout testing only.',
      'Workers inside iframes from other sites are not patched yet.',
      'The CDP port (`--cdp-port`) is refused while a profile has an identity.',
      'A patched Chromium (deeper fingerprint control) is planned for a later phase.',
    ),
  ),
);

export const adblock = chapter(
  'adblock',
  'Ad blocking, lighter pages and Reader',
  'The built-in ad blocker, per-site switches for JavaScript, images, fonts and more, and the reader view.',
  section(
    'what',
    'What it is',
    p(
      'Ad blocking uses Brave’s engine with the EasyList filter lists and blocks in the browser’s network layer, for every tab, iframe and worker. **Site policies** go further: per site (or for all sites) switch off JavaScript, images, web fonts, audio / video, requests to other sites, or animations. **Reader** pulls out the main text of an article.',
    ),
  ),
  section(
    'how',
    'How to use it',
    steps(
      'Open **Settings → Privacy & blocking**.',
      'Switch blocking on or off for everything; **Update lists** downloads fresh filter lists.',
      'Add sites where ads are allowed (for example a site you support).',
      'In the site policies table, click a cell to cycle it: inherited → blocked → allowed.',
      '**Check a site** shows which policy applies to an address.',
      'Your own filter rules go into `data\\filters\\user.txt` (Adblock Plus syntax).',
    ),
    settings('privacy'),
    // VERIFY-AFTER-MERGE: the "Site settings" section (permissions, zoom, policies per site).
    settings('sites', 'Everything about one site in one place (the “Site settings” section).'),
  ),
  section(
    'reader',
    'Reader',
    p('Reader extracts the main text of a page (readability.js) and can replace the page with a clean, readable version that follows the light / dark theme. There is no Reader button yet: ask the AI (“open this in reader view”) or call it:'),
    api('simplify.reader', { apply: true }),
  ),
  section(
    'ai',
    'For AI and scripts',
    api('adblock.toggle', { host: 'example.com', on: false }, 'Allow ads on one site:'),
    api('simplify.site', { host: 'news.example.com', js: false, images: false }),
    api('simplify.measure', { url: 'https://news.example.com' }, 'Measure how much a policy saves (bytes and load time):'),
    api('adblock.stats', {}),
  ),
  section(
    'limits',
    'Limits and known issues',
    list(
      'Switching JavaScript off breaks many modern sites; prefer it per site.',
      'Policies apply from the next page load: reload the tab.',
      'Hiding the empty ad boxes on a page (cosmetic filtering) is applied per tab with `adblock.apply`; blocking the requests works everywhere.',
    ),
  ),
);

export const performance = chapter(
  'performance',
  'Performance: tab freezing and memory',
  'The tab governor freezes and unloads background tabs and adapts limits to your RAM and CPU.',
  section(
    'what',
    'What it is',
    p(
      'The **governor** keeps the browser light when many tabs are open. A background tab is first **frozen** (the page is paused: no CPU, memory kept), later **discarded** (unloaded: memory freed; address, title and scroll position kept, it reloads when you open it). Under memory pressure the most idle tabs go first.',
    ),
    terms(
      ['low (≤ 8 GB RAM)', 'freeze after 2 min, discard after 10 min, at most 8 live tabs'],
      ['normal (~16 GB)', 'freeze after 5 min, discard after 30 min, at most 20 live tabs'],
      ['high (≥ 32 GB)', 'freeze after 10 min, discard after 60 min, at most 40 live tabs'],
    ),
  ),
  section(
    'how',
    'How to set it up',
    steps(
      'Open **Settings → Performance**.',
      'Switch the automatic policy on or off, pick a profile (it fills in the times), or set your own minutes and the number of live tabs.',
      'The live table (every 3 s) shows each tab’s state, idle time, JS memory and why a tab is left alone (the active tab, a tab an agent used recently…); below it, the recent decisions.',
      '**Reset to profile defaults** removes your own values.',
    ),
    settings('performance'),
  ),
  section(
    'ai',
    'For AI and scripts',
    api('tabs.freeze', { tab: 5, mode: 'discard' }),
    api('governor.status', {}),
    api('governor.config', { profile: 'normal', max_live_tabs: 15 }),
    api('governor.recommend_parallel', {}, 'How many parallel rows this computer can take now:'),
  ),
  section(
    'limits',
    'Limits and known issues',
    list(
      'A tab that got a command recently is never touched (agents keep their tabs alive).',
      'A discarded tab reloads when you open it: unsent form input there is lost.',
    ),
  ),
);

export const notifications = chapter(
  'notifications',
  'Notifications: Windows, Telegram, webhook',
  'Where the browser tells you that a run finished, an agent needs you or a patch waits — and questions you can answer with a button, also from your phone.',
  section(
    'what',
    'What it is',
    p('Events (a run finished, an agent asks, a self-healing patch is proposed…) become notifications on up to four channels:'),
    terms(
      ['Browser', 'a toast inside the browser window'],
      ['Windows', 'a system notification (toast) of Windows'],
      ['Telegram', 'a message from your own bot, with answer buttons'],
      ['Webhook', 'a POST with the notification as JSON to your URL'],
    ),
    p('**Questions** have buttons (for example Accept / Reject for a patch, or the choices of `ui.ask_user`) and can be answered in the browser or in Telegram; the first answer wins.'),
  ),
  section(
    'telegram',
    'Setting up Telegram',
    steps(
      'In Telegram, talk to @BotFather, create a bot and copy its token.',
      'Send any message to your new bot.',
      'Find your chat id: open `https://api.telegram.org/bot<token>/getUpdates` in a browser and read `chat.id`.',
      'In **Settings → Notifications** enter the token and the chat id, switch Telegram on and click **Test**.',
    ),
    warning('The bot token is stored in plain text in `data\\config\\notify.json`; the settings page shows it masked.'),
    settings('notifications'),
  ),
  section(
    'rules',
    'Rules and anti-spam',
    list(
      'The rules table decides, per event topic (`run.finished`, `run.*`, `*`), which channels get it and when: always, only when it failed, or only when the browser window is not focused.',
      'The first matching rule wins (order = priority); rules that can never match are flagged. “Which rule handles…” tells you which rule an event would hit.',
      'Anti-spam: the same text within 10 s is dropped, bundled rules collect events for 60 s, and each channel sends at most 20 per minute. Questions are never dropped.',
    ),
  ),
  section(
    'ai',
    'For AI and scripts',
    api('notify.send', { title: 'Report ready', body: '12 rows exported', level: 'success' }),
    api('ui.ask_user', { question: 'Publish the post?', choices: ['Publish', 'Not now'], timeout_secs: 600 }),
  ),
  section(
    'limits',
    'Limits and known issues',
    list(
      'Rejecting a patch from Telegram sends no feedback text (a button has no text field); give feedback in the browser.',
      'Existing `notify.json` files keep their rules: new default rules are offered with **Add missing default rules**.',
    ),
  ),
);

export const languages = chapter(
  'languages',
  'Languages',
  'The language of the browser’s interface, the languages websites see, and the language of Chromium’s own texts.',
  section(
    'what',
    'Three different languages',
    terms(
      ['Interface language', 'menus, panels, these settings and this guide (English or Ukrainian)'],
      ['Languages for websites', 'what sites see in `Accept-Language` and `navigator.languages`, so they pick their own language'],
      ['Browser language', 'Chromium’s built-in texts, such as some error pages and form hints'],
    ),
  ),
  section(
    'ui',
    'Interface language',
    p('**Settings → General → Language**: “Same as the system”, English or Українська. It applies at once in every window and page of the browser.'),
    settings('general'),
  ),
  section(
    'sites',
    'Languages for websites',
    p(
      'Each profile can send its own list, for example `uk-UA,uk,en`. A profile with an identity uses the identity’s languages (that is what keeps it consistent with its proxy).',
    ),
    // VERIFY-AFTER-MERGE: where the website-languages setting lives in the new sections.
    settings('profiles', 'Per-profile website languages.'),
    api('profile.create', { id: 'default', accept_language: 'uk-UA,uk,en-US,en' }),
  ),
  section(
    'browser',
    'Browser language',
    // VERIFY-AFTER-MERGE: browser (Chromium) language setting, restart behaviour, available locales.
    p(
      'Chromium’s own texts come from the language packs in the `locales\\` folder next to the exe (English and Ukrainian in the standard package; others fall back to English). A change applies after a restart of the browser.',
    ),
  ),
  section(
    'limits',
    'Limits and known issues',
    list(
      'Some parts of the interface are still English-only while they are being translated.',
      'Websites decide themselves whether they honour your language list.',
    ),
  ),
);

export const data = chapter(
  'data',
  'Data and privacy',
  'What is stored where in the data folder, what is unencrypted on purpose, site permissions, backups and moving the folder.',
  section(
    'layout',
    'The data folder',
    terms(
      ['`config\\`', 'settings per feature, plain JSON: `ai.json` (provider keys), `notify.json`, `governor.json`, `hints.json`, `identities.json`, `proxies.json`, `profiles.json`, `permissions.json`, `adblock.json`, `simplify.json`, `zoom.json`'],
      ['`profiles\\<id>\\`', 'the Chromium profile of one workspace: cookies, cache, local storage'],
      ['`db\\`', '`browsermcp.sqlite` (bookmarks, history, downloads, passwords), `runs.sqlite` (runs, rows, triggers)'],
      ['`filters\\`', 'ad-block lists, the compiled engine, `user.txt` (your own rules)'],
      ['`scripts\\`, `plugins\\`, `userscripts\\`', 'your automation code'],
      ['`runs\\<id>\\`', 'exports and downloads of a mass run'],
      ['`recordings\\`, `videos\\`, `har\\`, `pdf\\`, `mhtml\\`', 'what recording, traffic export and printing produce'],
      ['`downloads\\`', 'downloaded files'],
      ['`extract-cache\\`', 'selector plans of `page.extract`'],
      ['`logs\\`', '`cef.log` (the engine’s log)'],
      ['`runtime\\`', '`endpoint.json` (addresses and pid of the running browser), `session.json` (open tabs)'],
    ),
  ),
  section(
    'secrets',
    'What is unencrypted, by design',
    p(
      'You decided that the AI has **full access** and that secrets are not encrypted. So passwords (`db\\browsermcp.sqlite`), provider API keys (`config\\ai.json`), the Telegram token (`config\\notify.json`) and proxy logins (`config\\proxies.json`) are plain text. Anyone who can read the folder can read them.',
    ),
    list(
      'Keep the folder on your own Windows account (not on a shared drive or a USB stick you lend out).',
      'Treat a copy of the folder like a copy of all your passwords.',
      'Use incognito tabs or a separate profile for anything you do not want stored.',
    ),
  ),
  section(
    'permissions',
    'Site permissions',
    p(
      'When a site asks for the camera, microphone, notifications and so on, a bar asks you; “Remember” stores the answer as a rule (`config\\permissions.json`). Without a rule, tabs you cannot see (off-screen) are denied.',
    ),
    api('permissions.list', {}),
    api('permissions.set', { origin: 'https://meet.example.com', kind: 'camera', allow: true }),
    // VERIFY-AFTER-MERGE: the "Site settings" section.
    settings('sites'),
  ),
  section(
    'backup',
    'Backups and moving the folder',
    steps(
      'Close the browser (all windows), so no file is half-written.',
      'Copy or zip the whole `data\\` folder; that is the complete backup.',
      'To move: put the folder next to `browsermcp.exe` in the new place, or start with `--data-dir <path>`.',
      'To restore, replace the `data\\` folder with the backup while the browser is closed.',
    ),
    // VERIFY-AFTER-MERGE: the "Data" settings section (open folder, backup helpers).
    settings('data'),
    api('system.reveal_path', { path: 'D:\\BrowserMCP\\data\\recordings' }, 'Show a folder of the data folder in Explorer (an absolute path inside it):'),
  ),
  section(
    'limits',
    'Limits and known issues',
    list(
      'There is no built-in encryption and no sync.',
      'Only one running browser may use a data folder at a time.',
    ),
  ),
);

