# Daily-driver chrome: workspaces, split view, session restore, context menu, zoom, shortcuts, dropdowns, permission bar

Date: 2026-11-06. CEF 156 Alloy, Windows 11. Builds on `docs/phase2/tabs.md` and
`docs/phase2/profiles.md`. Code: `crates/cef/src/{chrome/,session.rs,menu.rs,zoom.rs,prompts.rs,
devtools.rs,clipboard.rs}`, `crates/core/src/{zoom.rs,commands/tabs.rs,commands/page.rs}`,
`crates/profiles/src/commands.rs`, `ui/src/lib/state/workspaces.ts`, `ui/src/lib/components/
{WorkspaceSwitcher,PermissionBar,PageSlot}.svelte`.

## What exists now

| Item | API / keys | How |
|---|---|---|
| Workspaces = profiles | `profile.list` (now with `label`, `color`), `profile.update {id, label?, color?}` (new), `profile.create/delete`, `tabs.open {profile}` | `TabInfo` carries `profile`, `incognito`, `offscreen`, `window`, `split`, `zoom`. The chrome URL has `?window=<key>`; each window's strip shows only its own tabs of the picked workspace and never off-screen tabs (they stay in `tabs.list` with `offscreen: true`). New tabs open in the workspace's profile; activating a tab of another workspace (agent, palette, restore) switches the window's workspace; closing a tab prefers a neighbour of the same profile. Create / rename / color / delete in the workspace dropdown (delete closes the workspace's tabs, then `profile.delete`; the launch profile cannot be deleted). |
| Split view | `tabs.split {left, right, ratio?}`, `tabs.unsplit {tab?}`, palette "Split view with the next tab", Ctrl+Alt+←/→ | The pages panel has no layout manager any more: `chrome::layout_pages` places the active tab, or both halves (`protocol::split_bounds`, 6 DIP gap). The UI view shows through the gap and draws the divider (`PageSlot`); dragging sends `bmcp://cmd/split?ratio=` (`&done=1` at the end → `tab.updated`). Activating either half shows both; the focused half is the active tab (`FocusHandler::on_got_focus` → `pane_focused`). Closing a half ends the split. Only content browsers vote in `can_close` (S1 rule unchanged). |
| Session restore | `--no-restore`; `tabs.restore_closed` (Ctrl+Shift+T, palette) | `data/runtime/session.json`, written 1 s after the last `tab.*` event (temp file + rename): per window the tabs (url, title, profile), active index, split. Never: incognito / off-screen / `about:blank` tabs, windows that are closing; a snapshot with no window is not written (quit keeps the last state). On start the launch profile's tabs open at once; tabs of other profiles wait for `bmcp_cef::profiles_applied()` (called by the app after `apply_stored`: proxy forwarders and identities first), and nothing is saved until they are back. Restored tabs keep their order whatever registers first (`session::insert_index`). Closed-tabs stack: 25 entries, in the same file, pruned when a profile is deleted. |
| Context menu | — | CEF's native menu (drawn by the OS over the page, never hidden under a view) with our model (`menu::build`): Back / Forward / Reload / Print, Open link in new tab, Open link in workspace ▸, Copy link address (Win32 clipboard), Open / Save image (into `data/downloads` via the download handler) / Copy image address, Undo…Select all in text fields, Copy for selections, "Ask AI about this" (`ui.ask_ai {tab, url, title, kind, text}` → the AI panel opens with a prepared draft), Inspect (DevTools in a window of their own: `ViewKind::DevTools`, never a tab, closed with the last window). |
| Zoom | `page.zoom {tab?, level? (percent) \| delta?}`, Ctrl+= / Ctrl+- / Ctrl+0 (+ numpad), Ctrl+wheel | `SetZoomLevel` on the tab and every open tab of the same host; per host in `data/config/zoom.json` (100 % = removed); applied on every main-frame commit. The omnibox shows the zoom when it is not 100 % (click = reset). Ctrl+wheel: Chromium zooms itself; on Ctrl key-up we read the shown tabs' zoom back and store it. |
| Native shortcuts | Ctrl+D, Ctrl+H, Ctrl+J, Ctrl+Shift+O, Ctrl+Shift+B, Ctrl+, (`settings.open`), Ctrl+= / - / 0, Ctrl+Alt+←/→ | `protocol::shortcut` (mirror of the UI keymap): work while a web page has focus. |
| Dropdowns over the page | `bmcp://cmd/cover?on=1\|0` | see "Decision" below. |
| Permission bar | `permission.prompt` / `permission.prompt_closed` events, `permissions.pending`, `permissions.answer {id, allow, remember?}` | A request no rule decides (`permissions::decision` = Ask) waits; the chrome shows "<origin> wants to …  ☑ Remember  [Allow] [Block]" for the tab on screen; remember stores `permissions.set` rules. Off-screen tabs: no rule = deny (as before). Waiting requests of a closing tab are denied. |

## Decision: dropdowns hide the page instead of an overlay view

Two options were on the table: (a) a generic popup `BrowserView` overlay (like the palette) that
renders dropdowns above the page, (b) make room for the dropdown in the chrome page itself. We
chose a variant of (b): **while any chrome dropdown is open, the pages panel is hidden**
(`chrome::set_cover`); the dropdown, drawn by the UI page, is then visible over the empty page
area. Why it is the more robust one:

* no second page with its own WS connection and state to keep in sync with the chrome (the
  palette has that cost already); the dropdown keeps the chrome's state, focus and keyboard;
* the page is not resized (no reflow, no `resize` events, scroll position kept) — shrinking the
  slot would relayout the page on every open;
* overlay views are opaque rectangles (S1 pitfall: no shadow / rounded corners) and need their
  own outside-click and focus-loss handling;
* any dropdown gets it with two lines (`chrome.dropdown(true/false)` in an `$effect`).

Cost: the page disappears while a menu is open (it is "hidden" for its own `visibilitychange`
too). Safety nets: `focus-content` and a reload of the chrome page uncover.

## Fix: bmcp://ui pages in content tabs never connected

`bmcp://ui/library.html` (and `settings.html`) shown in a **tab** stayed on "Connecting…": the
Local Network Access prompt for `bmcp://ui` → `127.0.0.1` was only granted to the chrome / palette
views; content tabs went through the web-page rules (deny). Now `scheme::allow_permission` is
asked first for every browser kind (same origin + only-loopback-bits check), so our own pages in a
tab reach the API, web pages still do not (they have another origin).

## Live checks (this machine, `--api-port 47895 --data-dir target/live-data --ui-dir ui/dist --cdp-port 9335`; UI driven through its CDP target, no desktop input)

| Check | Result |
|---|---|
| `tabs.open bmcp://ui/library.html` (content tab) | page text shows the Chrome profiles from `import.chrome_detect` (connected); log `permission prompt origin="bmcp://ui/" … allow=Some(true)` |
| `profile.create work` + `profile.update {label: "Work", color: "purple"}`, `tabs.open {profile: "work"}`, `tabs.open {window: "offscreen"}` | `tabs.list` rows carry `profile`/`window`, the off-screen one `offscreen: true`; the strip shows only the Work tab, the off-screen tab nowhere |
| Workspace dropdown (clicked via CDP) | lists "Personal 2 · Profile default · Direct", "Work 1 · …"; log `Cover(true)`; the page reports `visibilityState: hidden` while open; picking Personal shows its tabs, page visible again (1264×657) |
| New workspace "Shopping Test" in the dropdown form | profile `shopping-test` created, labelled, a new tab page opened in it and shown; delete (trash → Delete) closed the tab and removed the profile |
| `tabs.split {left: 3, right: 4}` | both pages visible 629 px wide (1264 − 6 gap) / 2, divider drawn at 629, both strip tabs show the split badge; `bmcp://cmd/split?ratio=0.3&done=1` → 377 / 881 px; `focus-pane?side=right` → active tab 4, its document has focus |
| Close one half (`tabs.close 4`) | split gone, the other page 1264 px; `tabs.restore_closed` → `{tab: 9}`, library back at index 1 |
| `page.zoom {delta: 1}` ×2 | 125 %, `devicePixelRatio` 1.25, `zoom.json` `{"example.com": 125.0}`, omnibox badge "125%"; a new `example.com/?second` tab opened at 125; badge click → 100 % on both, `zoom.json` `{}`; `window.bmcp.run('zoom.in')` (the native shortcut's code path) → 110 % |
| `getUserMedia({audio})` without a rule | bar "https://example.com wants to use your microphone"; Allow (remember) → page gets 1 track, rule stored; a second call succeeds without asking. `Notification.requestPermission()` → Block → `denied`, rule stored |
| Kill + restart (session restore) | same 4 tabs in the same order, the `work` tab restored after `profiles_applied` at its index, split 3 \| 5 at 0.4, active tab 3; incognito tab not in `session.json` |
| Off-screen tab right-click via CDP with `BMCP_MENU_AUTORUN` | menu: Open link in new tab, Open link in workspace ▸, Copy link address, Ask AI about this link, Inspect. Ask AI → AI panel opened with "About this link on "Example Domain" …"; Copy link → clipboard `https://iana.org/help/example-domains`; workspace → the link opened in profile `work`; Inspect → a `DevTools` browser (not a tab) |
| All tabs closed through the UI's WS (a person) | windows, DevTools and the off-screen tab close, `CEF shut down`, process gone; `session.json` keeps the last state |

## Known gaps

* **Ctrl+wheel zoom** is not verified: CDP-synthesized Ctrl+wheel does not zoom in Alloy, and
  real wheel input was not used (no desktop input). If Alloy does not zoom on Ctrl+wheel at all,
  the key-up read-back has nothing to read; Ctrl+= / Ctrl+- work.
* The native context menu of window tabs was not opened live (it is modal and needs a person to
  close it); its model and commands were exercised through off-screen tabs (`run_offscreen`).
  "Save image" was not run live (same download handler as other downloads).
* The split divider drag itself (pointer capture across the page views' HWNDs) was not
  dragged by hand; the drag's command (`split?ratio=`) was.
* Geolocation: `getCurrentPosition` produced no prompt in CEF 156 Alloy (pending forever) —
  Chromium's geolocation path does not reach `OnShowPermissionPrompt` here.
* Chromium delivers permission prompts only for the visible tab: a background tab's request
  waits until it is shown (then the bar appears).
* Session restore keeps URLs, not back/forward history; no pinned tabs (there is no pinning).
  Closing the last tab of a window closes the window; the session then keeps that tab.
* Workspace switching is per window in the UI only; the core has no "current workspace" (a new
  window starts in the launch profile's workspace unless its first tab is another profile's).
* `Ctrl+,` sends `settings.open` to the chrome; the page itself comes with the settings work.
* Workspace changes made over the API (profile.create/update by an agent) show up the next time
  the dropdown opens or a tab of that profile becomes active (there is no `profile.*` event).
