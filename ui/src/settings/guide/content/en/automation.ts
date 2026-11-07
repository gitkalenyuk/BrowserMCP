// Guide, English: traffic, recording, scripts / plugins / userscripts, mass runs.
// Facts: README.md, docs/phase2/profiles.md (CEF response limit), docs/phase7/*, docs/phase8/*,
// crates/jobs/README.md, crates/heal/README.md, crates/record/src/video.rs, scripts/examples.
import { api, chapter, cmd, code, key, keys, list, p, section, see, settings, steps, terms, tip, warning } from '../../model';

export const traffic = chapter(
  'traffic',
  'Traffic: requests, rules, breakpoints',
  'See every request of a tab with its body, export HAR, block / redirect / mock with rules, pause and edit requests, replay them and turn a site’s API into code.',
  section(
    'what',
    'What it is',
    p(
      'The Traffic panel ([[cmd:panel.traffic]]) records the network traffic of the active tab: every request with method, status, type, size and time, its headers and bodies, and WebSocket frames. On top of that you can change traffic: rules that apply automatically, and breakpoints that pause a request until you decide, like in Burp.',
    ),
  ),
  section(
    'why',
    'What it is for',
    list(
      'See which API calls a page makes and what they return, without DevTools.',
      'Save a HAR file for a bug report.',
      'Block a tracker or a broken script, redirect a script to a local copy, mock an API answer that the backend does not have yet.',
      'Pause a request, change a parameter or header, and send it on.',
      'Repeat a request with edits (`net.replay`), or let the browser write a typed TypeScript client from the calls it saw (`net.api_to_code`).',
    ),
  ),
  section(
    'how',
    'How to use it',
    steps(
      'Open the panel ([[cmd:panel.traffic]]) and switch **capture** on for the tab, then reload the page.',
      'Filter the list: words, `method:post`, `status:4xx`, `-word` to exclude, and the type chips (XHR, document, script…).',
      'Click a request for its headers, request body, response body and WebSocket frames.',
      '**Export HAR** writes a file to `data\\har\\` (the path is shown).',
      '**Rules** tab: add block / redirect / mock / rewrite-body rules by URL pattern.',
      '**Breakpoints** tab: pause matching requests; the editor shows the paused request, change what you need and **Continue**. A pause you leave alone continues unchanged after 60 seconds.',
    ),
    keys(cmd('panel.traffic', 'Traffic panel')),
  ),
  section(
    'ai',
    'For AI and scripts',
    p('The `net` toolset. Results are compact text made for agents; bodies come only on request (`net.get`).'),
    api('net.capture', { on: true }),
    api('net.list', {}),
    api('net.rule_add', { match: { url_glob: '*://ads.example.com/*' }, action: 'block' }),
    api('net.breakpoint', { match: { url_glob: '*/api/checkout*', method: 'POST' } }),
    api('net.har_export', {}),
    api('net.api_to_code', { host: 'api.example.com' }),
  ),
  section(
    'limits',
    'Limits and known issues',
    warning(
      'A response’s **status and headers cannot be changed**, only its body. This is a limit of CEF, not of BrowserMCP: CEF puts the network’s original headers back over whatever DevTools answers. So at the response stage `net.resume` refuses a new status or header change, and `net.rule_add` refuses `set_headers`, `remove_headers` and `mock`. Changing requests, and mocks answered at the request stage (instead of sending), work fully.',
    ),
    list(
      'Capture is per tab and has to be switched on; there is no command to read whether it is on.',
      'Bodies are fetched when you open a request; very old ones may have been evicted by the browser (use `net.capture {bodies: true}` to keep them).',
      'URLs longer than 120 characters are shortened in the list; the details show the full one.',
      'HAR files are written to `data\\har\\`; there is no “save as” dialog.',
    ),
  ),
);

export const recording = chapter(
  'recording',
  'Recording: actions, video, script',
  'Record what you (or an agent) do in a tab, with an optional video and traffic log, and turn the recording into a script.',
  section(
    'what',
    'What it is',
    p(
      'The Recording panel ([[cmd:panel.recording]]) records the steps done in the active tab — your clicks and typing as well as an agent’s commands — optionally with a video of the page and a log of its network traffic. A recording can be turned into a TypeScript script in one click.',
    ),
  ),
  section(
    'how',
    'How to use it',
    steps(
      'Open the tab you want to record and the panel ([[cmd:panel.recording]]).',
      'Choose the options (**video**, **network traffic**) and press record.',
      'Do the task. The steps appear live; typed passwords show as `***`.',
      'Press stop. The recording is saved in `data\\recordings\\<id>\\` (`actions.jsonl`, `meta.json`, and `video.webm` / `traffic.jsonl` when chosen).',
      'Open it and click **Generate script**. With “parametrize”, typed values become inputs (`inputs.email`) with the recorded value as default; secrets always become required inputs.',
      'Fill in the inputs form and **Save & run**: the script is saved and run once so you see that it works.',
    ),
    // VERIFY-AFTER-MERGE: the "Recording & scripts" settings section.
    settings('recording', 'Video format and other recording options (the “Recording & scripts” section).'),
    keys(cmd('panel.recording', 'Recording panel')),
  ),
  section(
    'video',
    'Video and ffmpeg',
    p(
      'Video is made from the page’s frames by **ffmpeg**, which is not bundled. The browser looks for it in this order: the `BMCP_FFMPEG` environment variable, `ffmpeg.exe` next to `browsermcp.exe`, then `PATH`.',
    ),
    code('powershell', 'winget install Gyan.FFmpeg'),
    p('Formats: WebM (VP9, default) or MP4 (H.264), 1–60 frames per second (default 10).'),
    api('record.video_start', { fps: 10, format: 'mp4' }),
    api('record.video_stop', {}),
  ),
  section(
    'ai',
    'For AI and scripts',
    api('record.start', { name: 'login', video: false, traffic: true }),
    api('record.stop', {}),
    api('record.codegen', { id: 'login', parametrize: true }),
  ),
  section(
    'limits',
    'Limits and known issues',
    list(
      'The panel records the active tab; there is no tab picker.',
      'Video needs ffmpeg (see above); without it the recording works, only the video is missing.',
      'Generated scripts are a starting point: add waits, loops or checks where the page needs them (or ask the AI to).',
    ),
  ),
);

export const scripts = chapter(
  'scripts',
  'Scripts, debugger, plugins and userscripts',
  'TypeScript automation with the same browser.* API as the SDK, a step debugger in Monaco, long-running plugins and userscripts on page loads.',
  section(
    'what',
    'What it is',
    p(
      'Scripts are TypeScript files in `data\\scripts\\`. They run in `bmcp-script-host.exe`, a separate V8 process, so a crashing script never takes the browser down. A script gets the typed `browser.*` object (every command), `inputs` (its input values, as strings), `browser.output({...})` for a structured result and `sleep(ms)`.',
    ),
    code(
      'ts',
      '// data/scripts/title.ts\nconst { tab } = await browser.tabs.open({ url: inputs.url });\nawait browser.nav.wait({ tab, for: "load" });\nconst t = (await browser.tabs.list()).tabs.find((x) => x.id === tab);\nbrowser.output({ url: inputs.url, title: t?.title });',
    ),
  ),
  section(
    'editor',
    'The Scripts panel and the debugger',
    steps(
      'Open the Scripts panel ([[cmd:panel.scripts]]): the list of saved scripts and an editor (Monaco, with completion and docs for every `browser.*` command).',
      'Save with [[Ctrl+S]]. **Run** asks for the inputs the code uses and shows the console and the result; an error’s line is a link.',
      'Click the gutter (or [[F9]]) to set breakpoints, then **Debug** ([[F5]]): the script stops there, showing the current line, variables and the call stack.',
    ),
    keys(
      cmd('panel.scripts', 'Scripts panel'),
      key('F9', 'Toggle a breakpoint'),
      key('F5', 'Debug / continue'),
      key('F10', 'Step over'),
      key('F11', 'Step into'),
      key('Shift+F11', 'Step out'),
      key('Shift+F5', 'Stop'),
    ),
  ),
  section(
    'plugins',
    'Plugins',
    p(
      'A plugin is a long-running script in `data\\plugins\\<name>\\` (`plugin.json` + `main.ts`). It runs in its own script host over a restricted connection: it may call only the commands and receive only the events its manifest lists, and it can add entries to the [[Ctrl+K]] palette.',
    ),
    code(
      'json',
      '{\n  "name": "hello-plugin",\n  "version": "0.1.0",\n  "main": "main.ts",\n  "permissions": ["tabs.list"],\n  "events": ["tab.updated"],\n  "ui": { "palette": [{ "id": "count-tabs", "title": "Hello plugin: count open tabs" }] }\n}',
    ),
    p('**Settings → Plugins & userscripts** installs a plugin from a folder, shows its status, permissions and logs, enables, disables and restarts it.'),
    settings('plugins'),
    api('plugins.install', { path: 'C:\\plugins\\hello-plugin' }),
  ),
  section(
    'userscripts',
    'Userscripts',
    p(
      'Userscripts in `data\\userscripts\\*.ts` have Tampermonkey-style headers (`// @match`, `// @run-at`) and run once per matching page load, as a script with `inputs.tab` and `inputs.url`. Code that must run inside the page goes through `browser.userscripts.inject`.',
    ),
    code('ts', '// ==UserScript==\n// @name     Example banner\n// @match    https://example.com/*\n// @run-at   document-end\n// ==/UserScript=='),
    p('Create, edit, enable and delete them in **Settings → Plugins & userscripts**.'),
  ),
  section(
    'ai',
    'For AI and scripts',
    api('script.save', { name: 'title', code: 'browser.output({ ok: true });' }),
    api('script.run', { name: 'title', inputs: { url: 'https://example.com' } }),
    api('script.debug_start', { name: 'title', breakpoints: [3] }),
  ),
  section(
    'limits',
    'Limits and known issues',
    list(
      '`bmcp-script-host.exe` must be next to `browsermcp.exe` (a package built with `--skip-script-host` cannot run scripts).',
      'Breakpoints only in the main script; stepping into an imported module shows its JavaScript lines.',
      'Variables are shown one level deep; no watch expressions or conditional breakpoints.',
      'The userscript editor is a plain text field.',
      'Unsaved edits of a script live in memory and are lost when the browser window reloads.',
    ),
  ),
);

export const runs = chapter(
  'runs',
  'Mass runs, triggers and self-healing',
  'Run a script once per row of a CSV / Excel / JSON file, in parallel profiles, with retries, exports, cron and webhook triggers, AI self-healing and Telegram approvals.',
  section(
    'what',
    'What it is',
    p(
      'A **run** executes one saved script for every row of your data: one script process per row, several at once, each row’s values as `inputs`. Everything is kept in `data\\db\\runs.sqlite`, so a run survives a browser restart.',
    ),
  ),
  section(
    'why',
    'What it is for',
    list(
      'Check 500 product pages, fill in 200 forms, collect prices from a list of shops.',
      'Log in with 50 test accounts, each in its own clean profile (and, if you want, its own proxy).',
      'Run a report every weekday at 9:00 (cron) or when another system calls a webhook.',
    ),
  ),
  section(
    'how',
    'How to start a run',
    steps(
      'Save a script (by hand or from a recording).',
      'Open the Runs panel ([[cmd:panel.runs]]) → new run.',
      'Pick the script and the data: a file path (`.csv`, `.json`, `.xlsx`) or rows pasted as CSV / JSON.',
      'Set parallel rows (default 4, limited by CPU cores and free memory), retries (default 1, with backoff), the timeout per row, what to do on selector errors, and whether to stop at the first failure.',
      'Watch the progress bar and the results table; filter by status, open a row for its error, inputs and output.',
      'Pause, resume or cancel at any time. **Export** to CSV, Excel, JSON, SQLite or a webhook.',
    ),
    keys(cmd('panel.runs', 'Runs panel')),
    tip('For rows that type or click in parallel, start the run with `profile: "isolated"`: every row gets a fresh in-memory profile and an off-screen tab, so rows never share cookies or focus.'),
  ),
  section(
    'heal',
    'Self-healing',
    p(
      'When a row breaks on a page selector (`NOT_FOUND`, `STALE_REF`, `NOT_INTERACTABLE`, `TIMEOUT`, `AMBIGUOUS_TARGET`), the run pauses and the AI proposes a patch with an explanation, a confidence and a diff. Accept it once and the rest of the run continues with the patched script (`login.v2`).',
    ),
    terms(
      ['`ask` (default)', 'pause, the AI proposes, you accept or reject (with feedback for the next attempt)'],
      ['`auto`', 'a patch with confidence ≥ 80 % that fixes the failing row in a dry run is applied without asking'],
      ['`off`', 'the row just fails'],
    ),
    p('You can also answer from Telegram (Accept / Reject buttons) or edit the patch by hand in the Runs panel.'),
    see('notifications', 'telegram'),
  ),
  section(
    'triggers',
    'Triggers: cron and webhooks',
    p('The Triggers tab of the Runs panel lists triggers with their next and last firing. A cron trigger has 5 fields in local time (minute hour day month weekday), for example `0 9 * * Mon-Fri`.'),
    warning('Weekday **numbers** follow the Rust cron library: 1 = Sunday … 7 = Saturday. `1-5` therefore means Sunday–Thursday; write `Mon-Fri` to be safe. The form shows the next five firings.'),
    api('trigger.set', { kind: 'cron', spec: '0 9 * * Mon-Fri', script: 'report', inputs: { to: 'me@example.com' } }),
    p('A webhook trigger gives you a URL `POST http://127.0.0.1:47800/v1/hooks/{id}`; the sender signs the body with the shared secret in the `X-BMCP-Signature: sha256=<HMAC>` header, and the JSON body becomes the script’s inputs.'),
    api('trigger.webhook_set', { script: 'report' }),
  ),
  section(
    'extract',
    'Extracting data with a cached plan',
    p(
      '`page.extract` returns data that matches a JSON Schema. The first time, the AI builds a selector plan and saves it in `data\\extract-cache\\`; later calls (for example in every row of a run) use the plan without AI, and rebuild it once if required fields come back empty.',
    ),
    api('page.extract', { schema: { type: 'object', properties: { title: { type: 'string' }, price: { type: 'number' } }, required: ['title'] } }),
  ),
  section(
    'ai',
    'For AI and scripts',
    api('run.start', { script: 'login', data: { csv_path: 'C:\\data\\logins.csv' }, parallel: 4, retries: 1, profile: 'isolated' }),
    api('run.status', { id: 'r1' }),
    api('run.export', { id: 'r1', to: 'xlsx', status: 'failed' }),
  ),
  section(
    'limits',
    'Limits and known issues',
    list(
      'Runs need `bmcp-script-host.exe` next to the browser.',
      'Export to Google Sheets is not implemented yet.',
      'Webhook triggers listen on the local API only (127.0.0.1); a sender on another computer needs a tunnel.',
      'The results table reloads at most once a second while a run is active; very large pages of results get slow.',
    ),
  ),
);
