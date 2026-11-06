# Phase 4 UI: library, bookmarks bar, Chrome import

Date: 2026-11-06. The daily-driver data of `crates/data` (bookmarks, history, downloads,
passwords, Chrome import) gets its UI: a full-tab library page `bmcp://ui/library.html`, the
omnibox star and a bookmarks bar in the chrome, and a first-run import wizard. Code map and
behaviour: `ui/README.md` ("Library").

## What is there

* **Library page** (`ui/library.html`, `ui/src/library/`): Bookmarks (folder tree, drag to
  reorder / move into folders, add / edit / delete, search), History (search, grouped by day,
  remove a page or a day, clear a range), Downloads (list with progress; "Show in folder" is a
  stub), Passwords (search, reveal after a confirmation for 30 s, copy, add, edit, delete),
  Import from Chrome (the wizard).
* **Chrome**: star in the omnibox (Ctrl+D), bookmarks bar under the toolbar (Ctrl+Shift+B), a
  Library button in the toolbar, a Library section in the menu, palette commands, Ctrl+Shift+O /
  Ctrl+H / Ctrl+J open the library at Bookmarks / History / Downloads (the section travels via
  localStorage, the URL has no query).
* **First run**: with no `bmcp.import.offered` flag and an empty database (no bookmarks,
  folders or history) the chrome opens the library at the import wizard once.
* **Native**: `crates/cef/src/scheme.rs` `is_allowed_in_content` also lets content tabs load
  exactly `bmcp://ui/library.html` (one line; no query or fragment accepted for it).
* **Contract fix** in `crates/app/src/main.rs`: `start_api` called `bmcp_api::serve(.., port, ..)`
  while still holding the listener bound before CEF, so the API never started ("Only one usage
  of each socket address"); it now calls `serve_on(.., listener, ..)` (one line).

## Verified

* `pnpm -C ui check` (0 errors, 0 warnings), `pnpm -C ui test` (new: `data/tree.test.ts`,
  `history.test.ts`, `wizard.test.ts`, `library.test.ts` — tree building, drop positions, day
  grouping across month ends, clear plans, the wizard state machine, first-run decision, composed
  edits against the stateful mock), `pnpm -C ui build` (4 entries).
* Mock (`pnpm dev`): every section, bookmarks bar folder click opening the library tab at that
  folder, Ctrl+H switching the open library to History, password reveal after the dialog, the
  wizard with a quoted pasted CSV path and per-part results.
* Real browser (`browsermcp --api-port 47862 --data-dir target/live-data --ui-dir ui/dist`),
  bookmarks / a folder / a login added over REST, history from `nav.goto`:
  * `tabs.open bmcp://ui/library.html` loads the library in a content tab (allow-list line);
    `bmcp://ui/library.html?api=ws&ws=ws://evil/` is refused (tab stays empty).
  * Library on the real core: bookmark tree with the REST folder, History grouped under Today,
    Passwords: eye → confirmation dialog → `passwords.list {origin, reveal:true}` shows the
    fixture password (driven with `page.act`, screenshots with `page.screenshot`).
  * Dev chrome against the real core (WS via a local Origin-stripping proxy, since only Vite on
    5173 is allowed and another agent held that port): the bar lists folder 1, the star on
    example.com called `bookmarks.add` (checked with `bookmarks.search`), the bar showed it, and
    Ctrl+H opened `bmcp://ui/library.html` as a real tab.
  * Not run: `import.chrome` against real Chrome data (by design; the wizard was exercised with
    the mock only). The first-run wizard did not open live because the start page
    (example.com) was already in history when the chrome checked — correct for that state.
  * Both browser instances shut down on their own after a while (`CEF shut down` in the log, no
    crash); the cause was not investigated.

## Screenshots

| | |
|---|---|
| ![bookmarks](img/live-library-bookmarks.jpg) | ![history](img/live-library-history.jpg) |
| real core: bookmark tree, folder from REST | real core: history grouped by day |
| ![confirm](img/live-password-confirm.jpg) | ![revealed](img/live-password-revealed.jpg) |
| real core: reveal needs a confirmation | revealed fixture login (hidden again after 30 s) |
| ![star](img/live-star-bar-devui.jpg) | ![bar](img/mock-bookmarks-bar.jpg) |
| dev chrome on the real core: star on, page in the bar | mock: bookmarks bar under the toolbar |
| ![folder](img/mock-library-folder.jpg) | ![wizard](img/mock-import-wizard.jpg) |
| mock: library opened from a bar folder | mock: import wizard, two Chrome profiles |
| ![done](img/mock-import-done.jpg) | ![reveal](img/mock-password-reveal.jpg) |
| mock: per-part import results | mock: reveal dialog |

## Core polish (same day, later)

The first three core gaps listed originally are closed:

* `bookmarks.update {id, title?, url?}` edits a bookmark or renames a folder **in place** (id
  and position stay); `ui/src/lib/data/edit.ts` uses it instead of add + move + remove.
  `bookmarks.add` and `bookmarks.update` accept only `http(s)://` and `file://` URLs
  (`javascript:` / `data:` are refused when written; Chrome bookmarklets are skipped on import).
* `history.delete {url?, since?, before?, day?}` deletes **visits** (in `[since, before)`, on the
  local day `YYYY-MM-DD`, and/or of one URL) and drops URLs left without visits. "Last hour …
  4 weeks" send `{since}`, "Remove day" sends `{day}`, so older visits of the same pages stay.
* Data events: the data commands emit `bookmarks.changed {op, id}` and `history.changed {op}`
  (also after `import.chrome`); both are in the WS server's default topics and the UI's
  `EXTRA_TOPICS`. The bar, the star and the library re-read on them (debounced); the
  localStorage tick and the window-focus refresh are gone.
* `system.reveal_path {path}` / `system.open_path {path}` (new crate `crates/system`): Explorer
  with the file selected / a folder opened, for paths inside the data folder or Downloads only
  (canonicalized first, so `..` and junctions cannot leave them; `open_path` refuses files,
  which Explorer would run). Downloads "Show in folder" and the recording's "Open folder"
  (`RecordingDetail.svelte`) use `reveal_path`.

Checked live (`browsermcp --api-port 47880 --data-dir target/live-data`, REST): `bookmarks.update`
kept id 3 / position 0 and changed title and URL; `javascript:` / `data:` URLs were refused; a
plain WS connection (default topics only) received `bookmarks.changed {op:"update", id:3}` and
`history.changed {op:"delete"}`; `history.delete {day}` removed that day's visit;
`system.reveal_path` opened Explorer on `live-data\videos` with `reveal check.txt` selected
(checked through the Shell COM API, window closed again); `..` out of the data folder,
`C:\Windows\System32\cmd.exe`, a UNC path and `open_path` on a file were refused.

## Gaps

* ~~Shortcuts while a web page has focus~~: Ctrl+D, Ctrl+H, Ctrl+J, Ctrl+Shift+O, Ctrl+Shift+B
  are in the native keymap mirror now (`crates/cef/src/chrome/protocol.rs`).
* ~~The menu's Library section is covered by the page~~: the page is hidden while a chrome
  dropdown is open (`docs/phase2/chrome-features.md`).
* Bookmarks-bar folders open the library at that folder instead of a dropdown (same reason).
  No drag and drop on the bar itself.
* Downloads: `downloads.list` is filled by a later CEF hook, so the live list is empty. The
  allowed Downloads folder is `%USERPROFILE%\Downloads`; a Downloads folder moved elsewhere
  (Windows known-folder redirect) is not followed, so files there cannot be revealed.
* Visits recorded automatically (the history recorder) emit no `history.changed`; an open
  History section shows new visits after its own actions, a search or a reconnect.
* With `--ui-dev` the chrome comes from Vite (another origin) and does not share localStorage
  with `bmcp://ui/library.html`, so the section request does not reach the page there.
