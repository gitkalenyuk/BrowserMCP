// Guide, English: troubleshooting and the glossary. Facts: xtask/src/dist.rs (packaged README),
// crates/app/src/main.rs (port error; release builds are windows_subsystem = "windows", so there
// is no console and the log on stderr is lost unless redirected), crates/app/src/stdio_proxy.rs,
// crates/record/src/video.rs. If the app gets its own log file or an error dialog, update the
// port / start / logs sections.
import { api, chapter, code, list, p, section, see, steps, terms } from '../../model';

export const troubleshooting = chapter(
  'troubleshooting',
  'Troubleshooting and FAQ',
  'SmartScreen, missing ffmpeg, a busy port, a browser that does not start, where the logs are, and other common questions.',
  section(
    'smartscreen',
    'Windows SmartScreen warns on start',
    p('The executables are not code-signed, so SmartScreen may show “Windows protected your PC”. Click **More info → Run anyway**. It usually asks only once per file.'),
  ),
  section(
    'ffmpeg',
    'Video recording says ffmpeg is missing',
    steps(
      'Install ffmpeg: `winget install Gyan.FFmpeg`, or download `ffmpeg.exe`.',
      'Either put `ffmpeg.exe` next to `browsermcp.exe`, set the environment variable `BMCP_FFMPEG` to its full path, or make sure it is on `PATH`.',
      'Restart the browser so it sees a changed `PATH`.',
      'Check it in **Settings → Recording & scripts**: **Check again** shows whether ffmpeg is found, and where.',
    ),
  ),
  section(
    'port',
    'The API port is busy',
    p('The browser window does not appear: the browser stopped because it could not get its API port. The packaged browser has no console window, so the reason is not shown on screen; when you capture it (see “Where are the logs?”) it reads “cannot use API port 47800 on 127.0.0.1 (another BrowserMCP may be running …)”. Usually another BrowserMCP already runs on the same data folder or port.'),
    list(
      'Look at `data\\runtime\\endpoint.json`: it holds the pid of the running instance. Close that browser, or end the process in Task Manager.',
      'Or start this one on another port: `browsermcp.exe --api-port 47801`, or `--api-port 0` for any free port. MCP clients then need that port (it is written to `endpoint.json`).',
    ),
  ),
  section(
    'start',
    'The browser does not start',
    list(
      'Unzip it into a folder you can write to (not “Program Files”) and keep all files together: the CEF files (`libcef.dll`, `*.pak`, `locales\\`…) must stay next to the exe.',
      'Only one browser per data folder: see “The API port is busy”.',
      'See why it stopped: the packaged browser has no console window, so its error messages are not shown anywhere. In PowerShell, in the browser’s folder, run `Start-Process .\\browsermcp.exe -RedirectStandardError err.txt -Wait` and read `err.txt` once it exits. For more detail run `$env:RUST_LOG="debug"` first.',
      'An MCP client that says “the browser did not start its API within 30 s” started `mcp-stdio`, which launches the browser in the background: start the browser by hand once to see the error.',
    ),
  ),
  section(
    'logs',
    'Where are the logs?',
    terms(
      ['`data\\runtime\\endpoint.json`', 'check it first: it is written once the API is up (pid and ports of the running browser) and removed on a normal exit. No file while the browser seems to run means its API did not start.'],
      ['`data\\logs\\cef.log`', 'the log of the Chromium engine'],
      ['The browser’s own messages', 'errors and the `RUST_LOG` log go to standard error, which the packaged build (no console window) shows nowhere. Capture them with `Start-Process .\\browsermcp.exe -RedirectStandardError err.txt -Wait` (`RUST_LOG`: `info` by default, `debug` for more). The errors of `browsermcp mcp-stdio` appear in the MCP client’s server log.'],
      ['Settings → About', '**Copy diagnostics** copies versions, folders and API addresses for a bug report (no browsing data); the **Logs** folder opens from there too.'],
      ['Plugins', 'Settings → Plugins & userscripts → logs, or `plugins.logs`'],
      ['Scripts', 'the console of the Scripts panel, and `stdout` / `error` in the result of `script.run`'],
    ),
  ),
  section(
    'scripts',
    'Scripts or runs do nothing',
    p('Scripts, runs, plugins and userscripts need `bmcp-script-host.exe` next to `browsermcp.exe`. A package built with `--skip-script-host` does not have it. **Settings → Recording & scripts** shows whether the script host was found.'),
  ),
  section(
    'ai',
    'The AI cannot read files or run commands',
    p(
      'An error like “Permission to use Read has been denied because Claude Code is running in don’t ask mode” means the chat runs in **Browser tools only** mode, where everything outside the browser’s own tools is refused. A chat keeps its mode: start a new one (**New** in the AI panel) with another **Permissions** choice such as **Ask me**, or change the default under **What CLI agents may do** in Settings → AI. For Codex, pick a wider **Sandbox** or **Approval requests** setting.',
    ),
    see('ai-panel', 'modes'),
  ),
  section(
    'faq',
    'Short answers',
    terms(
      ['Is it a Chrome extension?', 'No: a separate browser built on Chromium; Chrome extensions are not supported.'],
      ['Does it send my data anywhere?', 'Only where you connect it: AI providers you configure, your Telegram bot, your webhooks, proxies, filter list downloads and the IP lookup of `geo.lookup` (ip-api.com by default).'],
      ['Can a website control my browser through the API?', 'No: the API listens on 127.0.0.1 only and refuses requests from web pages (Host / Origin check).'],
      ['Can I use it from another computer?', 'Not yet; the API is local only.'],
      ['Does the AI cost money?', 'Cloud providers bill your own account as usual (local models such as Ollama cost nothing). Test buttons spend no tokens.'],
    ),
    api('system.ping', {}, 'Is the browser reachable?'),
    code('powershell', 'Invoke-RestMethod http://127.0.0.1:47800/v1/health'),
  ),
);

export const glossary = chapter(
  'glossary',
  'Glossary',
  'The words this guide and the API use, in one place.',
  section(
    'terms',
    'Terms',
    terms(
      ['MCP', 'Model Context Protocol: how AI agents such as Claude Code or Codex discover and call tools. BrowserMCP is an MCP server.'],
      ['Command', 'one browser function with typed arguments, such as `nav.goto`; the same over MCP (`nav_goto`), REST, WebSocket, SDK and scripts.'],
      ['Toolset', 'a group of commands (`core`, `net`, `record`…) that a client asks for.'],
      ['Snapshot', 'a compact text outline of a page from its accessibility tree.'],
      ['Ref', 'a short handle of one element in a snapshot, such as `@7:e3` (element 3, seen in snapshot 7); `@7:g1` is a collapsed group.'],
      ['Diff', 'what changed on a page since the last snapshot, returned after each action.'],
      ['Profile / workspace', 'a separate browser identity: cookies, cache, proxy, languages, fingerprint.'],
      ['Incognito', 'an in-memory profile that leaves nothing on disk.'],
      ['Off-screen tab', 'a tab without a window, for automation; input and screenshots still work.'],
      ['Headless', 'the browser started without any window (`--headless`).'],
      ['Agent group / lock', 'the colored tab group of one agent; its tabs are locked against other agents until you take over.'],
      ['Handoff', 'an agent asking you to do something in a tab (log in, CAPTCHA) and waiting for Done.'],
      ['Permission mode', 'what a Claude Code chat may do outside the browser’s tools: Ask me, Auto-accept edits, Full access, Plan only or Browser tools only. Codex has a sandbox and an approval setting instead.'],
      ['Approval card', 'a request of a CLI agent in the chat (“wants to run a command”) that waits for Allow once, Always allow or Deny.'],
      ['Identity / fingerprint', 'a consistent set of browser facts (user agent, screen, hardware, timezone…) that a profile shows to sites.'],
      ['Forwarder', 'a small local proxy that adds the login for an upstream proxy, so passwords never reach the engine.'],
      ['Pool / lease', 'a named list of proxies; a lease ties one proxy to one mass-run row.'],
      ['PAC rules', 'split routing: which addresses go through which proxy.'],
      ['Governor', 'the part that freezes and discards background tabs.'],
      ['Freeze / discard', 'pause a page (memory kept) / unload it (memory freed, reloads on open).'],
      ['Script host', '`bmcp-script-host.exe`, the separate process that runs TypeScript scripts.'],
      ['Run / row', 'a mass run of a script over data; one row = one script execution.'],
      ['Self-healing / patch', 'an AI fix for a script that broke on a page change, applied to the rest of a run.'],
      ['Trigger', 'something that starts a script by itself: cron (a schedule) or a webhook.'],
      ['Plugin', 'a long-running script with a permission manifest and palette entries.'],
      ['Userscript', 'a script that runs on matching page loads (`// @match`).'],
      ['HAR', 'HTTP Archive: a standard file with recorded network traffic.'],
      ['Breakpoint (traffic)', 'a rule that pauses matching requests until you edit and continue them.'],
      ['CDP', 'Chrome DevTools Protocol; with `--cdp-port` Playwright or Puppeteer can connect.'],
      ['CEF', 'Chromium Embedded Framework, the engine BrowserMCP is built on.'],
      ['Data folder', '`data\\` next to the exe: everything the browser stores.'],
    ),
  ),
);
