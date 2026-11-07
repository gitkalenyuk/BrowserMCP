// Guide, English: the built-in AI, connecting outside AI, how agents see pages.
// Facts: README.md, docs/phase5/panels.md, crates/ai, crates/api/README.md,
// crates/perception/README.md, crates/app/src/cli.rs.
import { api, chapter, cmd, code, keys, list, p, section, see, settings, steps, terms, tip, warning } from '../../model';

export const aiPanel = chapter(
  'ai-panel',
  'The built-in AI panel',
  'Chat with Claude Code, Codex or an API model right in the browser; providers, permission modes, approvals, chats and the terminal.',
  section(
    'what',
    'What it is',
    p(
      'The AI side panel ([[Ctrl+Shift+3]]) is a chat with an agent that can use the browser’s tools: it opens tabs, reads pages, clicks and types, watches traffic. Its tabs go into its own colored group, so you always see what it touches and can take over.',
    ),
    p('Three kinds of providers:'),
    list(
      '**Claude Code**: your installed `claude` command line, with your Claude login. BrowserMCP starts it with streaming JSON and connects its MCP tools automatically.',
      '**Codex**: your installed `codex` command line (`codex app-server`), the same way.',
      '**API models**: Anthropic, OpenAI, Google Gemini, OpenRouter, Mistral, DeepSeek, xAI, Groq, Ollama and LM Studio (local), and **Custom** for any OpenAI-compatible server. Here BrowserMCP runs the agent loop itself.',
    ),
  ),
  section(
    'why',
    'What it is for',
    list(
      '“Find the three cheapest flights to Lviv next Friday and open them in tabs.”',
      '“Fill in this form with the data from the PDF in my Downloads.”',
      'Right-click a page, a link or selected text → **Ask AI about this**: the panel opens with a prepared question.',
      'Background jobs use the default provider too: self-healing of mass runs and `page.extract`.',
    ),
  ),
  section(
    'providers',
    'Providers and keys',
    steps(
      'Open **Settings → AI** (or the gear in the AI panel).',
      'For Claude Code / Codex: install the command line tool and log in once in a terminal; set its path if it is not found automatically.',
      'For an API provider: paste the API key (and the base URL / model for Ollama, LM Studio or Custom).',
      'Click **Test**: it only lists models or runs `--version`, it spends no tokens.',
      'Pick **Use for new chats**, and **Make default for background AI** for self-healing and `page.extract`.',
    ),
    // VERIFY-AFTER-MERGE: several custom providers (add, name, save, remove).
    p(
      'You can add **several custom providers** (for example a local vLLM server and a company gateway): each gets its own name, base URL, key and model and is saved next to the presets.',
    ),
    warning('Keys are stored in plain text in `data\\config\\ai.json` (a deliberate choice). Anyone who can read the data folder can use them.'),
    settings('ai'),
  ),
  section(
    'modes',
    'Permission modes',
    // VERIFY-AFTER-MERGE: permission modes are being added; names from the brief, behaviour to check.
    p('The mode decides what a Claude Code or Codex agent may do on your computer besides the browser tools. Pick it in the chat before you send:'),
    terms(
      ['Ask me', 'Before anything not allowed yet (editing a file, running a command) the agent stops and asks you with an approval card.'],
      ['Auto-accept edits', 'File edits go through without asking; commands and everything else still ask.'],
      ['Full access', 'Everything is allowed without asking: files, commands, browser. Use it only for tasks and folders you trust.'],
      ['Plan only', 'The agent reads and plans, but changes nothing.'],
      ['Browser tools only', 'Only the BrowserMCP tools; no files and no shell.'],
    ),
    warning(
      'Earlier versions always ran Claude Code in “don’t ask” mode and Codex with a read-only shell, so any file or command tool was refused (“Permission … denied because Claude Code is running in don’t ask mode”). Pick “Ask me” or a wider mode when the task needs files.',
    ),
  ),
  section(
    'approvals',
    'Approval cards',
    // VERIFY-AFTER-MERGE: approval cards in the chat.
    p(
      'In “Ask me” mode a request shows up in the chat as a card: what the agent wants to do (the tool and its arguments, for example the command line or the file), with **Allow**, **Always allow** (for the rest of the chat) and **Deny**. The agent waits until you answer.',
    ),
  ),
  section(
    'chats',
    'Chats: continuing and closing',
    // VERIFY-AFTER-MERGE: continuing a chat with the next message, closing chats.
    list(
      'Every chat is a chip at the top of the panel; click one to switch.',
      'Your next message **continues the chat you have open**; start a new one with the **+ New** button.',
      'Close a chat with the × on its chip; a running answer is stopped first.',
      'The stop button ends the current answer, the chat stays.',
      'Tool calls are collapsible rows under the answer: click one to see what was sent and what came back.',
    ),
  ),
  section(
    'terminal',
    'Terminal',
    p(
      'The Terminal panel ([[Ctrl+`]]) is a real shell inside the browser: PowerShell (default), cmd or Git Bash, in tabs. Agents can open and use terminals too (`term.*`); their output is shown when you open the panel.',
    ),
    keys(cmd('panel.terminal', 'Terminal panel'), cmd('panel.ai', 'AI panel'), cmd('panel.close', 'Close the side panel')),
  ),
  section(
    'ai',
    'For AI and scripts',
    p('The same chats can be started over the API, for example by another agent or a cron script:'),
    api('ai.providers', {}),
    api('ai.session_start', { provider: 'claude-code', prompt: 'Open example.com and summarize it', toolsets: ['core'] }),
    api('term.open', { shell: 'pwsh' }),
  ),
  section(
    'limits',
    'Limits and known issues',
    list(
      'Claude Code and Codex need their command line tools installed and logged in; their usage counts against your own Claude / OpenAI account.',
      'Ollama and LM Studio count as configured even when their local server is not running: use **Test**.',
      'An agent gets the `core` toolset by default; pass more toolsets when it needs traffic, recording and so on.',
    ),
  ),
);

export const connect = chapter(
  'connect',
  'Connecting an outside AI or program',
  'Claude Code, Codex or any MCP client over HTTP or stdio; toolsets; REST, WebSocket, the TypeScript SDK and the CDP port for Playwright.',
  section(
    'what',
    'What it is',
    p(
      'While the browser runs, it serves every command on `127.0.0.1:47800` — local only, no tokens. Requests from web pages are refused (a Host / Origin check), so a website cannot drive your browser.',
    ),
    terms(
      ['`http://127.0.0.1:47800/mcp`', 'MCP (Streamable HTTP); add `?toolsets=core,net,…` for more tool groups'],
      ['`http://127.0.0.1:47800/v1`', 'REST: `POST /v1/cmd/{name}`, `GET /v1/commands`, `GET /v1/health`'],
      ['`http://127.0.0.1:47800/openapi.json`', 'OpenAPI 3.1 description of every command'],
      ['`ws://127.0.0.1:47800/v1/ws`', 'WebSocket: JSON-RPC 2.0 calls plus event notifications'],
    ),
    p('The running browser writes these addresses and its process id to `data\\runtime\\endpoint.json`.'),
  ),
  section(
    'claude',
    'Claude Code',
    p('Over HTTP (the browser must be running):'),
    code('bash', 'claude mcp add --transport http browsermcp http://127.0.0.1:47800/mcp'),
    p('Over stdio (this also starts the browser when it is not running):'),
    code('bash', 'claude mcp add browsermcp -- C:\\path\\to\\BrowserMCP\\browsermcp.exe mcp-stdio'),
    p('Then ask, for example: “open example.com and click the link”. Tools are named like the commands with `_` instead of the dot: `nav_goto`, `page_snapshot`.'),
  ),
  section(
    'codex',
    'Codex',
    p('Add the browser to `~/.codex/config.toml`:'),
    code('toml', '[mcp_servers.browsermcp]\ncommand = \'C:\\path\\to\\BrowserMCP\\browsermcp.exe\'\nargs = ["mcp-stdio"]'),
  ),
  section(
    'other',
    'Any other MCP client',
    p('Point it at `http://127.0.0.1:47800/mcp` (Streamable HTTP), or run `browsermcp.exe mcp-stdio` as a stdio server. `mcp-stdio --headless` starts a browser without windows when none is running.'),
    // VERIFY-AFTER-MERGE: the "Connections & API" settings section (status, copy-ready commands).
    settings('connections', 'Addresses, status and ready-to-copy commands (the “Connections & API” section).'),
  ),
  section(
    'toolsets',
    'Toolsets',
    p(
      'Commands are grouped into toolsets, so an agent sees only what it needs (fewer tools = fewer tokens and fewer mistakes). MCP lists only `core` and `system` by default.',
    ),
    terms(
      ['`core`', 'tabs, navigation, page snapshot / find / act / read, keys, dialogs, handoff'],
      ['`net`', 'traffic: capture, HAR, rules, breakpoints, replay, API → code'],
      ['`record`', 'recording, codegen, video'],
      ['`script`', 'scripts, debugger, `page.extract`, plugins, userscripts'],
      ['`run`', 'mass runs, self-healing, cron and webhook triggers'],
      ['`profile`', 'profiles, cookies, site permissions'],
      ['`stealth`', 'identities, fingerprints, proxies, devices, human-like input'],
      ['`data`', 'bookmarks, history, downloads, passwords, import, PDF, ad blocking'],
      ['`ai`', 'built-in AI sessions and terminals'],
      ['`ui`', 'notifications, questions to the user, hints, find in page'],
      ['`system`', 'ping, agents, tab governor, open a file or folder'],
    ),
    code('bash', 'claude mcp add --transport http browsermcp "http://127.0.0.1:47800/mcp?toolsets=core,system,net,record"'),
    tip('Over stdio the default toolsets are used; connect over HTTP when you need more.'),
  ),
  section(
    'rest',
    'REST and WebSocket',
    p('REST: the body is the command’s arguments as JSON; errors come back with an HTTP status and `{code, message, retry, hint}`. Every example in this guide has a PowerShell and a curl variant.'),
    api('tabs.open', { url: 'https://example.com' }),
    p('WebSocket: `method` is the command name; subscribe to events with `events.subscribe`.'),
    code(
      'json',
      '-> {"jsonrpc":"2.0","id":1,"method":"tabs.list","params":{}}\n-> {"jsonrpc":"2.0","id":2,"method":"events.subscribe","params":{"topics":["tab.*","run.*"]}}\n<- {"jsonrpc":"2.0","method":"tab.updated","params":{"id":3,"title":"Example Domain","seq":12}}',
    ),
  ),
  section(
    'sdk',
    'TypeScript SDK',
    p('`@browsermcp/sdk` (Node 22+) gives typed `browser.*` calls and events. It is not published yet: use it from the source workspace after `pnpm -r build`.'),
    code(
      'ts',
      'import { connect } from "@browsermcp/sdk";\n\nconst browser = await connect(); // ws://127.0.0.1:47800/v1/ws\nconst { tab } = await browser.tabs.open({ url: "https://example.com" });\nawait browser.nav.wait({ tab, for: "load" });\nconsole.log((await browser.page.snapshot({ tab })).text);\nbrowser.on("tab.*", (data, topic) => console.log(topic, data));',
    ),
  ),
  section(
    'cdp',
    'CDP port for Playwright and Puppeteer',
    p('Start the browser with `--cdp-port 9222` to open Chromium’s DevTools port, then connect with `connectOverCDP`:'),
    code('ts', 'import { chromium } from "playwright";\n\nconst browser = await chromium.connectOverCDP("http://127.0.0.1:9222");\nconst page = browser.contexts()[0].pages()[0];'),
    warning(
      'The CDP port is off by default and is refused while any profile has a stealth identity (an open debugging port is easy to detect). It also exposes the browser’s own pages with full API rights to any local program, so only open it when you need it.',
    ),
  ),
  section(
    'agents',
    'Several agents at once',
    p('Each connected agent gets its own colored tab group and locks the tabs it works in; another agent touching them gets `TAB_LOCKED`. You can take a tab over at any time.'),
    api('agents.list', {}),
    api('tabs.lock', { tab: 3 }),
    see('tabs', 'groups'),
  ),
  section(
    'limits',
    'Limits and known issues',
    list(
      'Local only: there is no network access from other computers yet (a later “worker mode” with TLS and a key is planned).',
      'If port 47800 is busy (another BrowserMCP is running), the browser does not start: use `--api-port <n>` or `--api-port 0`.',
    ),
    see('troubleshooting', 'port'),
  ),
);

export const perception = chapter(
  'perception',
  'How an AI sees pages',
  'Snapshots of the accessibility tree with refs, find, diff after actions and reading text — and why screenshots are the exception.',
  section(
    'what',
    'What it is',
    p(
      'Instead of screenshots, an agent asks for a **snapshot**: a compact text outline of the page built from Chromium’s accessibility tree. Every interactive element gets a **ref** such as `@7:e3`, and the agent acts on that ref.',
    ),
    code(
      'text',
      'snap 7 tab=3 "Sign in – Acme" https://acme.com/login vp=1280x720 y=0/1840\n- main\n  - heading "Sign in" h1\n  - textbox "Email" @7:e3 req\n  - textbox "Password" @7:e4 pwd\n  - button "Sign in" @7:e6 disabled\n  - list "Providers" 12 items, 3 shown → @7:g1',
    ),
  ),
  section(
    'why',
    'Why not screenshots',
    list(
      'Text costs far fewer tokens than an image of the same page, and it is faster to produce and to read.',
      'A ref points at exactly one element: no guessing coordinates, no clicking next to the button.',
      'States are explicit: `disabled`, `req`, `on`/`off`, `expanded`, `invalid`, `focused`.',
      'Passwords are never printed (a password field shows `pwd`).',
      '`page.screenshot` still exists for canvas, images or layout questions — on request only.',
    ),
  ),
  section(
    'how',
    'How an agent works with it',
    steps(
      '`page.snapshot` → the outline with refs (interactive mode by default; `full` adds text; `skeleton` only the outline, deeper parts become `→ @S:gN` to drill into with `root`).',
      '`page.find` → elements by role, name, text or a regex, when the page is big.',
      '`page.act` → click, fill, select, check, press, scroll… on a ref. It waits until the element can really be used.',
      'The answer of `page.act` is a **diff**: one line of what changed (`+ alert "Wrong password"`, `~ @8:e6 enabled`), not a whole new snapshot.',
      '`page.read` → the visible text of the page or of one element, for articles, messages and tables.',
    ),
    api('page.snapshot', { mode: 'interactive' }),
    api('page.find', { role: 'button', text: 'sign in' }),
    api('page.act', { ref: '@7:e3', action: 'fill', value: 'me@example.com' }),
    api('page.diff', {}),
    api('page.read', { max_chars: 4000 }),
  ),
  section(
    'refs',
    'Refs and errors',
    list(
      '`@S:eN`: element N, seen in snapshot S. It stays valid while the document lives; after a navigation old refs give `STALE_REF`.',
      'Refs are unique across tabs: a ref from another tab never matches by accident.',
      'Big pages fall back to a skeleton within ~1500 tokens; long lists show 3 items and a `→ @S:gN` group.',
      'Iframes (also from other sites) and shadow DOM are part of the snapshot.',
    ),
    p('Errors are made for agents: `{code, message, retry, hint, candidates?}`, with codes such as `STALE_REF`, `AMBIGUOUS_TARGET`, `NOT_INTERACTABLE`, `TAB_LOCKED`, `TIMEOUT`, and `retry` telling whether to try again now, after a wait, after a new snapshot or not at all.'),
  ),
  section(
    'limits',
    'Limits and known issues',
    list(
      'Canvas-drawn apps (maps, games, some editors) have little in the accessibility tree: use `page.screenshot` there.',
      '`page.read` cannot look inside closed shadow roots (their controls still work by ref).',
      'Very deep pages are cut at 512 levels.',
    ),
  ),
);

