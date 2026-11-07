// Guide, English: getting started and everyday browsing. Facts come from README.md,
// docs/phase2/*, docs/phase4/library.md, ui/README.md and the code they name.
import { api, chapter, cmd, code, key, keys, list, p, section, see, settings, steps, tip, warning } from '../../model';

export const start = chapter(
  'start',
  'Getting started',
  'What BrowserMCP is, where it keeps your data, the first start and moving over from Chrome.',
  section(
    'what',
    'What BrowserMCP is',
    p(
      'BrowserMCP is a desktop browser built on Chromium (through CEF, the Chromium Embedded Framework) that you use every day **and** that AI agents can drive: Claude Code, Codex or any model that speaks MCP. The browser chrome (tabs, address bar, panels, these settings) is our own interface; the pages themselves are rendered by Chromium, so sites work as in Chrome.',
    ),
    p(
      'Every function of the browser is one **command** in a single registry, for example `tabs.open`, `page.snapshot` or `net.capture`. From that registry the browser serves the same commands over MCP (for AI agents), REST and WebSocket (for programs), a TypeScript SDK and the `browser.*` object of automation scripts. What you can click, an agent can call, and the other way round.',
    ),
    p(
      'Agents read pages as compact text built from the accessibility tree, with short references (`@7:e3`) to every button and field, not as screenshots. That is faster, cheaper in tokens and more reliable.',
    ),
    tip('BrowserMCP runs on Windows only for now (a macOS version is planned later).'),
  ),
  section(
    'why',
    'What it is for',
    list(
      'Let an AI do web chores in **your** browser, with your logins: fill in forms, compare prices, collect data from pages, check a site after a deploy.',
      'Record what you do once, turn it into a script and run it 1000 times with different data (mass runs), with retries, reports and exports.',
      'Debug websites and APIs: see every request with its body, pause and edit requests like Burp, mock answers, export HAR, turn a site’s API into TypeScript code.',
      'Keep separate identities apart: profiles with their own cookies, proxy, language and browser fingerprint.',
      'Browse lighter: ad blocking, per-site switches for JavaScript / images / fonts, a reader view and automatic tab freezing.',
    ),
  ),
  section(
    'folder',
    'The portable data folder',
    p(
      'BrowserMCP is portable. Unzip it anywhere you can write to and start `browsermcp.exe`. Everything it stores lives in the `data\\` folder next to the executable: profiles, cookies, history, passwords, scripts, recordings, logs. Nothing is written to `%APPDATA%` or the registry.',
    ),
    list(
      'To move the browser, close it and move or copy the whole folder.',
      'To uninstall, delete the folder.',
      'To keep the data somewhere else, start it with `--data-dir D:\\other-data`.',
      'Only one running browser can use a data folder at a time. `data\\runtime\\endpoint.json` tells tools where the running instance listens.',
    ),
    warning('Do not unzip into "Program Files": the browser must be able to write its `data\\` folder next to the exe.'),
    see('data', 'layout', 'What exactly is in the data folder.'),
  ),
  section(
    'first-run',
    'First start',
    steps(
      'Unzip the archive, for example to `D:\\BrowserMCP`, and double-click `browsermcp.exe`.',
      'If Windows SmartScreen warns about an unknown app, click **More info → Run anyway**: the executables are not code-signed.',
      'The browser opens with one window. With an empty history it offers to import from Chrome once (you can skip it).',
      'Pick your language in **Settings → General → Language** ([[cmd:settings.open]] opens Settings).',
      'Connect your AI: open the AI panel ([[cmd:panel.ai]]) to chat with the built-in agent, or connect Claude Code / Codex from outside.',
    ),
    settings('general', 'Language, theme, tab layout.'),
    see('connect'),
    code(
      'powershell',
      'browsermcp.exe --help                     # all options\nbrowsermcp.exe https://github.com --profile work\nbrowsermcp.exe --data-dir D:\\other-data   # keep the data somewhere else\nbrowsermcp.exe --api-port 0               # pick a free API port (default 47800)\nbrowsermcp.exe --no-restore               # do not reopen the last session',
    ),
  ),
  section(
    'import',
    'Moving over from Chrome',
    p(
      'The import wizard brings over bookmarks, history, passwords and cookies. Every part is optional and importing the same files twice adds nothing the second time.',
    ),
    steps(
      'Open the wizard: menu **Library → Import from Chrome…**, or [[Ctrl+K]] and type “import”.',
      'Pick the Chrome profile (the wizard lists the ones it finds).',
      '**Bookmarks and history** are read straight from the Chrome profile folder. Chrome may stay open: history is copied first, the original is never touched.',
      '**Passwords**: in Chrome open `chrome://password-manager` → Settings → Export passwords, save the CSV, then give its path to the wizard.',
      '**Cookies**: a JSON file written by a cookie-export extension. Imported cookies are applied to the profile when the import finishes and at the next start.',
      'Read the per-part results at the end.',
    ),
    warning(
      'Passwords and cookies are stored unencrypted in the data folder (your choice when the project started). Delete the exported CSV afterwards: it holds all your passwords in plain text.',
    ),
    api('import.chrome_detect', {}, 'An agent can find the Chrome profiles first:'),
    api('import.chrome', { parts: ['bookmarks', 'history'] }),
  ),
  section(
    'limits',
    'Limits and known issues',
    list(
      'Windows only for now.',
      'The executables are not code-signed, so SmartScreen may warn on the first start.',
      'The Chromium sandbox is off in this build (docs/release.md explains why).',
      'There is no sync between computers: copy the data folder instead.',
    ),
  ),
);

export const tabs = chapter(
  'tabs',
  'Tabs, workspaces and sessions',
  'Tabs and agent groups, split view, workspaces (profiles), incognito tabs and session restore.',
  section(
    'what',
    'What it is',
    p(
      'Tabs work as in Chrome. On top of that: a vertical tab bar, split view (two tabs side by side), **workspaces** (each one a separate profile with its own cookies, proxy and identity) and **agent groups**: every AI agent that works in the browser gets its own colored group of tabs that it locks while it works.',
    ),
  ),
  section(
    'how',
    'Everyday use',
    list(
      'New tab [[cmd:tab.new]], close [[cmd:tab.close]], reopen the last closed tab [[cmd:tab.reopen]] (up to 25 are remembered).',
      'Vertical or horizontal tabs: [[cmd:ui.toggleVerticalTabs]] or Settings → General.',
      'Zoom with [[cmd:zoom.in]] / [[cmd:zoom.out]] / [[cmd:zoom.reset]]; it is remembered per site and the address bar shows it when it is not 100 % (click it to reset).',
      'Right-click a page for Back / Reload / Print, open a link in a new tab or in another workspace, copy or save images, **Ask AI about this** and **Inspect** (DevTools in a window of its own).',
      'When a site asks for the camera, microphone or notifications, a bar asks you: Allow / Block, with “Remember”.',
    ),
  ),
  section(
    'groups',
    'Agent groups and taking over',
    p(
      'Tabs an agent opens or acts on are locked for that agent: other agents get a `TAB_LOCKED` error, and the tab’s group label shows the agent’s color and a lock. You can always step in: the banner over the page offers **Take over**. When an agent needs you (log in, solve a CAPTCHA), it asks with a banner and waits until you click **Done**.',
    ),
    api('tabs.groups', {}, 'List the groups and their tabs:'),
    api('ai.handoff', { reason: 'Log in, then click Done', wait_secs: 300 }, 'What an agent calls to hand a tab to you:'),
  ),
  section(
    'split',
    'Split view',
    p(
      'Two tabs side by side in one window. Palette ([[Ctrl+K]]): “Split view with the next tab” splits the active tab with its neighbour; the same entry ends the split. Drag the divider to resize. Closing either half ends the split.',
    ),
    keys(cmd('split.focusLeft', 'Focus the left half'), cmd('split.focusRight', 'Focus the right half'), cmd('tab.split', 'Split with the next tab / end the split')),
    api('tabs.split', { left: 3, right: 4, ratio: 0.5 }),
  ),
  section(
    'workspaces',
    'Workspaces and profiles',
    p(
      'A workspace is a **profile**: its own cookies, cache, logins, proxy, website language and optional fingerprint identity, stored in `data\\profiles\\<id>\\`. The workspace menu on the toolbar lists them; create, rename, recolor and delete them there. Each window shows the tabs of the workspace you picked; new tabs open in it.',
    ),
    list(
      'Typical use: “Personal”, “Work”, “Client A”: separate logins on the same sites.',
      'Deleting a workspace closes its tabs and deletes the profile. The profile the browser started with cannot be deleted.',
      'Start the browser in another profile: `browsermcp.exe --profile work`.',
    ),
    api('profile.create', { id: 'work', accept_language: 'de-DE,de,en' }),
    api('tabs.open', { url: 'https://example.com', profile: 'work' }),
    see('stealth', 'identity', 'Proxies and fingerprints per profile.'),
  ),
  section(
    'incognito',
    'Incognito tabs',
    p(
      'An incognito tab uses an in-memory profile: nothing is written to disk, and everything is gone when its last tab closes. Incognito tabs are never saved in the session.',
    ),
    api('tabs.open', { url: 'https://example.com', incognito: true }),
    warning('There is no incognito button in the browser yet: open such tabs through the AI, a script or the API.'),
  ),
  section(
    'restore',
    'Session restore',
    p(
      'The browser remembers your windows and tabs (address, title, profile, the active tab, split view) in `data\\runtime\\session.json` and reopens them on the next start. Back/forward history of a tab is not kept. Start with `--no-restore` to skip it once.',
    ),
    // VERIFY-AFTER-MERGE: the "Startup & tabs" settings section and what it offers.
    settings('startup', 'What to open on start (the “Startup & tabs” section).'),
  ),
  section(
    'keys',
    'Shortcuts',
    keys(
      cmd('tab.new', 'New tab'),
      cmd('tab.close', 'Close tab'),
      cmd('tab.reopen', 'Reopen closed tab'),
      cmd('tab.next', 'Next tab'),
      cmd('tab.prev', 'Previous tab'),
      cmd('tab.select1', 'First tab (Ctrl+2 … Ctrl+8 for the others)'),
      cmd('tab.selectLast', 'Last tab'),
      cmd('ui.toggleVerticalTabs', 'Vertical / horizontal tabs'),
      cmd('zoom.in', 'Zoom in'),
      cmd('zoom.out', 'Zoom out'),
      cmd('zoom.reset', 'Reset zoom'),
    ),
  ),
  section(
    'ai',
    'For AI and scripts',
    p('Agents see every tab with `tabs.list` (with its profile, window and whether it is off-screen) and open tabs that are invisible to you with `window: "offscreen"` — that is how parallel runs work without taking over your screen.'),
    api('tabs.list', {}),
    api('tabs.open', { url: 'https://example.com', window: 'offscreen' }),
    api('tabs.restore_closed', {}),
  ),
  section(
    'limits',
    'Limits and known issues',
    list(
      'Tab groups are agent groups; you cannot create your own groups or pin tabs yet.',
      'Session restore keeps addresses, not the back/forward history.',
      'An automation call never closes your last window: it opens a new tab page there first.',
      'Ctrl+mouse wheel zoom is not verified in this engine build; the keys work.',
      'Workspace changes made by an agent show the next time the workspace menu opens.',
    ),
  ),
);

export const keyboard = chapter(
  'keyboard',
  'Address bar, palette, shortcuts and link hints',
  'The omnibox, the Ctrl+K command palette, every keyboard shortcut and how to change them, and Vim-style link hints.',
  section(
    'what',
    'What it is',
    p(
      'BrowserMCP can be used almost without the mouse: the **omnibox** opens addresses and searches, the **command palette** ([[Ctrl+K]]) finds any command by name, Chrome’s shortcuts work, and **link hints** put a letter label on every link and button so you can click by typing.',
    ),
  ),
  section(
    'omnibox',
    'Omnibox (address bar)',
    list(
      'Type an address (`github.com`) and press Enter: it opens with https:// (http:// for `localhost` and IP addresses).',
      'Type words: they are searched with Google.',
      '`javascript:` typed in the address bar is searched, never run.',
      'The star (or [[cmd:bookmark.toggle]]) bookmarks the page into the bookmarks bar, or removes its bookmarks.',
    ),
    keys(cmd('omnibox.focus', 'Focus the address bar'), cmd('bookmark.toggle', 'Bookmark / un-bookmark the page')),
    // VERIFY-AFTER-MERGE: the "Search" settings section (search engine choice).
    settings('search', 'The search engine (the “Search” section).'),
  ),
  section(
    'palette',
    'Command palette',
    p(
      '[[Ctrl+K]] opens a search box over the page. Type part of a name (“split”, “history”, “traffic”) and press Enter. It lists the browser’s commands with their shortcuts, and the entries that running plugins add.',
    ),
    keys(cmd('palette.open', 'Open / close the palette'), key('Esc', 'Close it')),
  ),
  section(
    'shortcuts',
    'Keyboard shortcuts',
    p(
      'Shortcuts follow Chrome’s. Letters are matched by key position, so [[Ctrl+T]] also works with the Ukrainian layout. The keys below are your current ones.',
    ),
    keys(
      cmd('find.open', 'Find in page'),
      cmd('nav.back', 'Back'),
      cmd('nav.forward', 'Forward'),
      cmd('nav.reload', 'Reload'),
      cmd('library.bookmarks', 'Bookmarks manager'),
      cmd('library.history', 'History'),
      cmd('library.downloads', 'Downloads'),
      cmd('ui.toggleBookmarksBar', 'Show / hide the bookmarks bar'),
      cmd('panel.traffic', 'Traffic panel'),
      cmd('panel.recording', 'Recording panel'),
      cmd('panel.ai', 'AI panel'),
      cmd('panel.terminal', 'Terminal panel'),
      cmd('panel.scripts', 'Scripts panel'),
      cmd('panel.runs', 'Runs panel'),
      cmd('settings.open', 'Settings'),
    ),
    steps(
      'Open **Settings → Keyboard**.',
      'Search by words or by an exact shortcut such as `ctrl+shift+t`.',
      'Click **+** on a command and press the new keys; remove a key with its ×; reset one command or all.',
      'A key bound twice is shown in red: the command later in the list wins.',
    ),
    settings('keyboard'),
  ),
  section(
    'hints',
    'Link hints (Vim style)',
    p(
      'Press [[f]] on a page (not inside a text field): every link, button and field gets a short letter label. Type the label to click it. [[F]] (Shift+F) opens the picked link in a new background tab. Letters filter, [[Backspace]] corrects, [[Esc]] cancels.',
    ),
    keys(
      key('f', 'Show hints: click'),
      key('Shift+F', 'Show hints: open in a new background tab'),
      key('Backspace', 'Undo the last typed letter'),
      key('Esc', 'Hide the hints'),
    ),
    list(
      'In **Settings → Link hints** set a key per mode (click, new tab, focus, copy link), the label letters (default `asdfghjkl`) and sites where the keys stay off (for example a web mail with its own shortcuts).',
      'Hints can also be opened from the palette: “Show link hints”.',
    ),
    settings('hints'),
  ),
  section(
    'ai',
    'For AI and scripts',
    api('hints.show', { mode: 'click' }),
    api('page.find_text', { query: 'price' }, 'Find in page (Ctrl+F) from the API:'),
    api('input.keys', { keys: 'Tab Tab Enter' }, 'Press keys in a tab:'),
  ),
  section(
    'limits',
    'Limits and known issues',
    list(
      'Changed shortcuts are stored in the local storage of the browser’s interface (`bmcp.keymap.overrides`), not yet in `data\\config\\keymap.json`.',
      'While a web page has focus, the browser uses its built-in copy of the **default** keys: your changes apply in the browser’s own interface, not yet inside pages.',
      '[[Ctrl+K]], [[Esc]] and [[Ctrl+L]] / [[Alt+D]] / [[F6]] are handled by the browser itself and cannot be changed.',
    ),
  ),
);

export const library = chapter(
  'library',
  'Bookmarks, history, downloads and passwords',
  'The library page, the bookmarks bar, history search and cleanup, downloads and the password manager.',
  section(
    'what',
    'What it is',
    p(
      'The library is a full-tab page (`bmcp://ui/library.html`) with five parts: Bookmarks, History, Downloads, Passwords and Import from Chrome. Open it from the toolbar’s folder button, the menu, the palette or the shortcuts below. Everything is stored in `data\\db\\browsermcp.sqlite`.',
    ),
  ),
  section(
    'bookmarks',
    'Bookmarks',
    list(
      'The star in the address bar ([[cmd:bookmark.toggle]]) adds the page to the bookmarks bar or removes it.',
      'The bookmarks bar ([[cmd:ui.toggleBookmarksBar]]) shows the first folder; a folder on it opens in the library.',
      'In the library: a folder tree, drag rows to reorder or onto a folder to move, add / edit / delete, search.',
      'Only http(s) and file addresses can be bookmarked; bookmarklets (`javascript:`) are skipped on import.',
    ),
    api('bookmarks.add', { url: 'https://example.com', title: 'Example' }),
    api('bookmarks.search', { q: 'docs' }),
  ),
  section(
    'history',
    'History',
    p(
      'History is grouped by day and searchable. Remove one page (all its visits), one day, or a range: the last hour … 4 weeks, older than 4 weeks or everything. Removing deletes visits; a page keeps the visits outside the range.',
    ),
    api('history.search', { q: 'invoice' }),
    api('history.delete', { day: '2026-11-06' }),
  ),
  section(
    'downloads',
    'Downloads',
    p(
      'Downloads start without a dialog and go to `data\\downloads\\` (files of a mass run go to `data\\runs\\<id>\\downloads\\`). The library lists them with progress; **Show in folder** opens Explorer with the file selected.',
    ),
    // VERIFY-AFTER-MERGE: the "Downloads" settings section (folder choice, ask where to save).
    settings('downloads', 'Where downloads go (the “Downloads” section).'),
    api('downloads.list', {}),
  ),
  section(
    'passwords',
    'Passwords',
    p(
      'The password manager keeps logins per site. The list never shows passwords: the eye asks you to confirm, then shows that one password for 30 seconds. Add, edit (an empty password field keeps the current one), copy and delete.',
    ),
    warning(
      'Passwords are stored **unencrypted** in `data\\db\\browsermcp.sqlite`, and the AI has full access to them. That was a deliberate choice; keep the data folder somewhere only you can read.',
    ),
    api('passwords.list', { origin: 'https://github.com' }),
  ),
  section(
    'keys',
    'Shortcuts',
    keys(
      cmd('library.bookmarks', 'Bookmarks'),
      cmd('library.history', 'History'),
      cmd('library.downloads', 'Downloads'),
      cmd('library.passwords', 'Passwords'),
      cmd('bookmark.toggle', 'Bookmark this page'),
      cmd('ui.toggleBookmarksBar', 'Bookmarks bar'),
    ),
  ),
  section(
    'ai',
    'For AI and scripts',
    p('Every library change made by you, an agent, a script or an import sends `bookmarks.changed` / `history.changed` events, so open pages refresh by themselves.'),
    api('data.top_sites', {}),
    api('page.pdf', {}, 'Print the current page to PDF (saved in `data\\pdf\\`):'),
  ),
  section(
    'limits',
    'Limits and known issues',
    list(
      'Bookmarks-bar folders open the library instead of a drop-down; no drag and drop on the bar itself.',
      'Downloads only stay tracked while their tab is open; closing the tab right after a download starts can leave it “in progress”.',
      '“Show in folder” works for the data folder and your Windows Downloads folder only (a moved Downloads folder is not followed).',
    ),
  ),
);

export const newtab = chapter(
  'newtab',
  'New tab and appearance',
  'The new tab page with its widgets, and how the browser looks: theme, accent color, density, tab layout.',
  section(
    'what',
    'What it is',
    p(
      'The new tab page is a small dashboard of widgets you can switch on and off (more will be added): **Ask the AI** (a box that starts a chat), **Agents & runs** (what agents and mass runs are doing, patches waiting for your OK), **Quick launch scripts** and **Top sites**.',
    ),
  ),
  section(
    'how',
    'How to set it up',
    steps(
      'Open a new tab ([[cmd:tab.new]]).',
      'Click **Customize** and switch widgets on or off; or use Settings → General → New tab widgets.',
      'Type a task into **Ask the AI**, pick the agent and send it: a new agent session starts. Its tabs appear in its own group; follow it in the AI panel.',
    ),
    settings('general'),
  ),
  section(
    'look',
    'Appearance',
    list(
      '**Theme**: System (follows Windows light / dark), Light or Dark. The palette has “Toggle light / dark theme”.',
      '**Accent color**: buttons, the selected tab and focus rings; pick a swatch or any custom color.',
      '**Density**: Compact makes the tab strip, toolbar and tabs shorter.',
      '**Vertical tabs** ([[cmd:ui.toggleVerticalTabs]]) and the **bookmarks bar** ([[cmd:ui.toggleBookmarksBar]]).',
    ),
    p('These settings are saved on this computer and apply to every window at once.'),
  ),
  section(
    'limits',
    'Limits and known issues',
    list('Widgets are fixed to the four above; there is no widget API for plugins yet.'),
  ),
);
