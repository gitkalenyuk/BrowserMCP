//! The composed browser window: our Svelte chrome around the pages of several tabs.
//!
//! ```text
//! Window (no layout manager: we place the two children ourselves)
//! ├─ UI view        bmcp://ui/index.html, fills the WHOLE window (bottom of the z-order).
//! │                 Draws tab strip, toolbar, optional vertical tabs / side panel, and
//! │                 leaves a transparent "page slot" whose rectangle it reports.
//! ├─ pages panel    placed exactly over that page slot (on top of the UI view).
//! │  └─ one content BrowserView per tab (FillLayout); only the active one is visible
//! └─ overlay        Window::add_overlay_view(palette view, CUSTOM docking), Ctrl+K
//! ```
//!
//! * The UI reports the page slot with `bmcp://cmd/layout?x&y&w&h&vw&vh`; we turn it into
//!   insets from the window edges ([`protocol::Insets`]), so a window resize moves the page
//!   at once, and vertical tabs / side panels just work (the slot simply moves).
//! * Each window gets a numeric key stored as the Window's view id (`Window::set_id`).
//!   Any handler finds its window from a `Browser` with [`window_key`].
//! * Tab order + active tab per window: `ChromeWindow::strip` (a `bmcp_core::TabStrip`).
//!   The tab lifecycle (pending → registered → closed) is described in `bridge.rs`.
//! * Closing: one tab of several → `close_browser(false)` → `DoClose` returns true and we
//!   remove its view from the pages panel ([`detach_tab`]). The last tab → the window
//!   closes (only content browsers vote in `can_close`, see the S1 pitfalls).

pub(crate) mod protocol;

use std::cell::RefCell;

use bmcp_core::{BackendError, NEW_TAB_URL, TabId, TabStrip};
use cef::*;

use crate::bridge::{PendingTab, ShellContext, next_view_id, post_to_ui, with_ui};
use crate::profiles::ProfileKey;
use crate::scheme::{PALETTE_PAGE, UI_PAGE, ui_page_url};
use protocol::{Bounds, Command, Insets, PaletteAction, Shortcut};

/// What a browser is used for. Decides which handlers apply and whether it is a tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ViewKind {
    /// A web page. Registered as a tab; visible to MCP.
    Content,
    /// Our chrome (tabs, toolbar, panels): fills the window behind the pages.
    Ui,
    /// The Ctrl+K palette overlay.
    Palette,
}

/// A UI or palette browser (never a tab).
pub(crate) struct ChromeView {
    pub kind: ViewKind,
    pub browser: Browser,
}

/// The CEF views of one window. Cheap to clone (CEF references) — copy them out of
/// `UiState` before calling CEF.
#[derive(Clone)]
pub(crate) struct Views {
    pub window: Window,
    pub ui: BrowserView,
    pub pages: Panel,
    pub palette: BrowserView,
    pub overlay: Option<OverlayController>,
}

/// One composed window: its views plus tab state.
pub(crate) struct ChromeWindow {
    pub views: Views,
    pub strip: TabStrip,
    /// Where the page area is, as distances from the window edges.
    pub insets: Insets,
    /// Last known client size (from `on_layout_changed`), DIP.
    pub size: (i32, i32),
    /// Set once the user (or `quit`) asked to close the whole window.
    pub closing: bool,
}

/// Views created before the window exists; moved into the window in `on_window_created`.
/// (`Clone` is required by the `wrap_window_delegate!` macro.)
#[derive(Clone)]
struct Parts {
    ui: BrowserView,
    pages: Panel,
    palette: BrowserView,
    first_tab: BrowserView,
}

/// Key of the composed window that hosts `browser`, if any.
pub(crate) fn window_key(browser: &Browser) -> Option<i32> {
    let mut browser = browser.clone();
    browser_view_get_for_browser(Some(&mut browser))
        .and_then(|view| view.window())
        .map(|window| window.id())
        .filter(|id| *id != 0)
}

fn views(key: i32) -> Option<Views> {
    with_ui(|ui| ui.windows.get(&key).map(|w| w.views.clone()))
}

/// The BrowserView that shows tab `id`.
fn tab_view(id: TabId) -> Option<BrowserView> {
    let mut browser = with_ui(|ui| ui.tabs.get(&id).map(|t| t.browser.clone()))?;
    browser_view_get_for_browser(Some(&mut browser))
}

fn run_js(view: &BrowserView, code: &str) {
    if let Some(frame) = view.browser().and_then(|b| b.main_frame()) {
        frame.execute_java_script(Some(&CefString::from(code)), None, 0);
    }
}

fn cef_rect(b: Bounds) -> Rect {
    Rect {
        x: b.x,
        y: b.y,
        width: b.width,
        height: b.height,
    }
}

// ------------------------------------------------------------------ opening tabs

/// Create a content view for `pending` (the browser itself appears later, see bridge.rs).
fn create_content_view(ctx: &ShellContext, pending: PendingTab) -> Option<BrowserView> {
    let view = crate::shell::create_browser_view(
        pending.initial_url(),
        ViewKind::Content,
        ctx,
        &pending.profile,
    )?;
    remember_pending(&view, pending);
    Some(view)
}

/// Give `view` a unique id and remember what it is for.
fn remember_pending(view: &BrowserView, pending: PendingTab) {
    let view_id = next_view_id();
    view.set_id(view_id);
    with_ui(|ui| ui.pending.insert(view_id, pending));
}

/// Open a composed window whose first tab is `pending`.
pub(crate) fn open_window(pending: PendingTab, ctx: &ShellContext) {
    let key = next_view_id();
    let pending = PendingTab {
        window: Some(key),
        index: None,
        activate: true,
        ..pending
    };
    let Some(first_tab) = create_content_view(ctx, pending) else {
        return;
    };
    let ours = ProfileKey::default(); // our own views always use the global context
    let ui = crate::shell::create_browser_view(&ui_page_url(UI_PAGE), ViewKind::Ui, ctx, &ours);
    let palette = crate::shell::create_browser_view(
        &ui_page_url(PALETTE_PAGE),
        ViewKind::Palette,
        ctx,
        &ours,
    );
    let pages = panel_create(None);
    let (Some(ui), Some(palette), Some(pages)) = (ui, palette, pages) else {
        tracing::error!("could not create the chrome views");
        return;
    };
    pages.set_to_fill_layout();
    let parts = Parts {
        ui,
        pages,
        palette,
        first_tab,
    };
    let mut delegate = ChromeWindowDelegate::new(key, RefCell::new(Some(parts)), ctx.style);
    window_create_top_level(Some(&mut delegate));
}

/// `tabs.open`: a new tab in the window used last (or a new window if there is none).
pub(crate) fn open_tab(pending: PendingTab) -> Result<(), BackendError> {
    let ctx = with_ui(|ui| ui.shell.clone()).ok_or(BackendError::ShuttingDown)?;
    let key = with_ui(|ui| {
        ui.last_window
            .filter(|k| ui.windows.contains_key(k))
            .or_else(|| ui.windows.keys().next_back().copied())
    });
    match key {
        Some(key) => add_tab(&ctx, key, pending),
        None => open_window(pending, &ctx),
    }
    Ok(())
}

/// Add `pending` as a new tab of window `key` (at `pending.index`, None = end).
fn add_tab(ctx: &ShellContext, key: i32, pending: PendingTab) {
    let Some(views) = views(key) else { return };
    let activate = pending.activate;
    let pending = PendingTab {
        window: Some(key),
        ..pending
    };
    let Some(view) = create_content_view(ctx, pending) else {
        return;
    };
    insert_view(&views, &view, activate);
}

/// Add `pending` to window `key` right after tab `after` (a re-opened popup).
pub(crate) fn add_tab_after(ctx: &ShellContext, key: i32, after: TabId, pending: PendingTab) {
    let index = with_ui(|ui| ui.windows.get(&key)?.strip.index_of(after).map(|i| i + 1));
    add_tab(ctx, key, PendingTab { index, ..pending });
}

/// Put a content view into the pages panel. Background tabs start hidden; a foreground
/// tab is shown (and the old one hidden) by `show_tab` once its browser exists.
fn insert_view(views: &Views, view: &BrowserView, activate: bool) {
    if !activate {
        view.set_visible(0);
    }
    views.pages.add_child_view(Some(&mut View::from(view)));
    if !activate {
        // The layout skips hidden views: give the page the real size anyway, so a
        // background tab has a layout (boxes, scroll sizes) before it is first shown.
        size_like_panel(views, view);
    }
}

/// Bounds of the pages panel (0,0-based) for one of its child views.
fn size_like_panel(views: &Views, view: &BrowserView) {
    let panel = views.pages.bounds();
    if panel.width > 0 && panel.height > 0 {
        view.set_bounds(Some(&Rect {
            x: 0,
            y: 0,
            width: panel.width,
            height: panel.height,
        }));
    }
}

/// After a resize: hidden tabs follow the panel size too (see [`insert_view`]).
fn size_hidden_tabs(key: i32, views: &Views) {
    let hidden: Vec<TabId> = with_ui(|ui| {
        let Some(w) = ui.windows.get(&key) else {
            return Vec::new();
        };
        let active = w.strip.active();
        w.strip
            .tabs()
            .iter()
            .copied()
            .filter(|id| Some(*id) != active)
            .collect()
    });
    for id in hidden {
        if let Some(view) = tab_view(id) {
            size_like_panel(views, &view);
        }
    }
}

/// A page opened a popup (`window.open`, `target=_blank`, middle click): make it a tab
/// right after its opener. Returns false if the opener is not in a composed window.
pub(crate) fn adopt_popup(opener: &Browser, popup: &BrowserView) -> bool {
    let opener_id = TabId(opener.identifier());
    let placement = with_ui(|ui| {
        let (background, url) = ui
            .popups
            .remove(&opener.identifier())
            .unwrap_or((false, String::new()));
        let opener_tab = ui.tabs.get(&opener_id)?;
        let (profile, run) = (opener_tab.profile.clone(), opener_tab.run.clone());
        let key = opener_tab.window?;
        let w = ui.windows.get(&key)?;
        let index = w.strip.index_of(opener_id).map(|i| i + 1);
        Some((key, w.views.clone(), index, background, url, profile, run))
    });
    let Some((key, views, index, background, url, profile, run)) = placement else {
        return false;
    };
    // The popup already loads its URL in the opener's request context (same profile).
    remember_pending(
        popup,
        PendingTab {
            window: Some(key),
            index,
            activate: !background,
            url,
            reply: None,
            profile,
            run,
            then_load: None,
        },
    );
    insert_view(&views, popup, !background);
    true
}

// ------------------------------------------------------------------ switching tabs

/// Show tab `id` in window `key`: hide the previously shown one, focus the page (or the
/// omnibox on the new tab page), update the window title, emit `tab.activated`.
pub(crate) fn show_tab(key: i32, id: TabId) {
    let state = with_ui(|ui| {
        let w = ui.windows.get_mut(&key)?;
        let previous = w.strip.active();
        w.strip.activate(id).then(|| (w.views.clone(), previous))
    });
    let Some((views, previous)) = state else {
        return;
    };
    let Some(view) = tab_view(id) else { return };
    view.set_visible(1);
    if let Some(prev) = previous.filter(|p| *p != id).and_then(tab_view) {
        prev.set_visible(0);
    }
    let url = with_ui(|ui| ui.tabs.get(&id).map(|t| t.url.clone())).unwrap_or_default();
    if url.starts_with(NEW_TAB_URL) {
        focus_omnibox(&views);
    } else {
        view.request_focus();
    }
    update_window_title(key);
    crate::bridge::emit_activated(id);
}

/// `tabs.activate`.
pub(crate) fn activate_tab(id: TabId) -> Result<(), BackendError> {
    let key = with_ui(|ui| ui.tabs.get(&id).map(|t| t.window));
    match key {
        None => Err(BackendError::TabNotFound(id)),
        Some(Some(key)) => {
            show_tab(key, id);
            Ok(())
        }
        Some(None) => Ok(()), // single layout: every tab is its own window
    }
}

/// Window title = title of the shown tab.
pub(crate) fn update_window_title(key: i32) {
    let state = with_ui(|ui| {
        let w = ui.windows.get(&key)?;
        let title = w
            .strip
            .active()
            .and_then(|id| ui.tabs.get(&id))
            .map(|t| t.title.clone())
            .unwrap_or_default();
        Some((w.views.window.clone(), title))
    });
    let Some((window, title)) = state else { return };
    let title = if title.is_empty() {
        "BrowserMCP".to_owned()
    } else {
        format!("{title} — BrowserMCP")
    };
    window.set_title(Some(&CefString::from(title.as_str())));
}

/// A tab's title changed: update the window title if it is the shown tab.
pub(crate) fn tab_title_changed(id: TabId) {
    let key = with_ui(|ui| {
        let key = ui.tabs.get(&id)?.window?;
        (ui.windows.get(&key)?.strip.active() == Some(id)).then_some(key)
    });
    if let Some(key) = key {
        update_window_title(key);
    }
}

// ------------------------------------------------------------------ closing tabs

/// What `tabs.close` from an automation caller does with a tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeepWindowPlan {
    /// Close it as usual (other tabs remain, or it has no window: off-screen).
    Close,
    /// It is the last tab of a composed window: open the new tab page there first.
    ReplaceThenClose(i32),
    /// Single layout (every tab is its own window): show the new tab page instead.
    ShowNewTabInstead,
}

/// Decide [`KeepWindowPlan`] (pure): `window` = (key, tabs in it) of a composed window.
pub(crate) fn keep_window_plan(window: Option<(i32, usize)>, offscreen: bool) -> KeepWindowPlan {
    match window {
        Some((key, tabs)) if tabs <= 1 => KeepWindowPlan::ReplaceThenClose(key),
        Some(_) => KeepWindowPlan::Close,
        None if offscreen => KeepWindowPlan::Close,
        None => KeepWindowPlan::ShowNewTabInstead,
    }
}

/// The plan for tab `id` (see [`keep_window_plan`]). UI thread.
pub(crate) fn plan_keep_window(id: TabId) -> Result<KeepWindowPlan, BackendError> {
    let facts = with_ui(|ui| {
        let tab = ui.tabs.get(&id)?;
        let window = tab
            .window
            .and_then(|key| ui.windows.get(&key).map(|w| (key, w.strip.tabs().len())));
        Some((window, tab.offscreen))
    });
    let (window, offscreen) = facts.ok_or(BackendError::TabNotFound(id))?;
    Ok(keep_window_plan(window, offscreen))
}

/// Open the new tab page as a tab of window `key`; `reply` learns its id once it exists.
pub(crate) fn open_new_tab_in(
    key: i32,
    reply: tokio::sync::oneshot::Sender<TabId>,
) -> Result<(), BackendError> {
    let ctx = with_ui(|ui| ui.shell.clone()).ok_or(BackendError::ShuttingDown)?;
    let mut pending = PendingTab::new(NEW_TAB_URL, ProfileKey::default(), None);
    pending.reply = Some(reply);
    add_tab(&ctx, key, pending);
    Ok(())
}

/// Single layout: load the new tab page in tab `id` instead of closing its window.
pub(crate) fn show_new_tab(id: TabId) -> Result<(), BackendError> {
    let browser = with_ui(|ui| ui.tabs.get(&id).map(|t| t.browser.clone()))
        .ok_or(BackendError::TabNotFound(id))?;
    if let Some(frame) = browser.main_frame() {
        frame.load_url(Some(&CefString::from(NEW_TAB_URL)));
    }
    Ok(())
}

/// `tabs.close`: ask the page to close (it may run `beforeunload`). What happens next is
/// decided in [`do_close`].
pub(crate) fn close_tab(id: TabId) -> Result<(), BackendError> {
    let entry = with_ui(|ui| {
        ui.tabs
            .get(&id)
            .map(|t| (t.browser.clone(), t.window, t.offscreen))
    });
    let (browser, window, offscreen) = entry.ok_or(BackendError::TabNotFound(id))?;
    match window {
        // Composed window, or a windowless tab (no view, no window: close the browser).
        Some(_) => {
            if let Some(host) = browser.host() {
                host.close_browser(0);
            }
        }
        None if offscreen => {
            if let Some(host) = browser.host() {
                host.close_browser(0);
            }
        }
        // Single layout: a Views browser must be closed through its window (S2 pitfall).
        None => {
            let mut b = browser.clone();
            if let Some(window) =
                browser_view_get_for_browser(Some(&mut b)).and_then(|v| v.window())
            {
                window.close();
            }
        }
    }
    Ok(())
}

/// `LifeSpanHandler::do_close` for a content browser: its page agreed to close.
/// Returns 1 when we take care of it (one tab of several: remove its view), 0 to let CEF
/// close the whole window (last tab, or the whole window is closing anyway).
pub(crate) fn do_close(browser: &Browser) -> i32 {
    let id = TabId(browser.identifier());
    let state = with_ui(|ui| {
        let key = ui.tabs.get(&id)?.window?;
        let w = ui.windows.get(&key)?;
        let others: Vec<Browser> = w
            .strip
            .tabs()
            .iter()
            .filter(|t| **t != id)
            .filter_map(|t| ui.tabs.get(t).map(|e| e.browser.clone()))
            .collect();
        Some((key, w.closing, others))
    });
    let Some((key, closing, others)) = state else {
        return 0; // single layout or not ours: default behaviour
    };
    if others.is_empty() {
        // The last tab: the window goes with it.
        tracing::info!(%id, window = key, "last tab of the window closed: window closes");
        return 0;
    }
    let others_ready = || {
        others
            .iter()
            .all(|b| b.host().is_some_and(|h| h.is_ready_to_be_closed() == 1))
    };
    if closing && others_ready() {
        return 0; // the whole window is closing and every other tab agreed too
    }
    // Remove the view after this callback returns (not from inside CEF's close path).
    post_to_ui(Box::new(move || detach_tab(key, id)));
    1
}

/// Take a closing tab out of its window: show its neighbour, then remove the view from
/// the pages panel. Dropping the last reference destroys the view, which destroys the
/// browser (`on_before_close` → `bridge::unregister_tab`).
fn detach_tab(key: i32, id: TabId) {
    let Some(view) = tab_view(id) else { return };
    let state = with_ui(|ui| {
        let w = ui.windows.get_mut(&key)?;
        let next = w.strip.remove(id).and_then(|r| r.new_active);
        // Re-checked after every removal: when several tabs close at the same time, each
        // `do_close` saw the others as "still there" and none let the window close.
        let empty = w.strip.tabs().is_empty();
        Some((w.views.pages.clone(), w.views.window.clone(), next, empty))
    });
    let Some((pages, window, next, empty)) = state else {
        return;
    };
    if let Some(next) = next {
        show_tab(key, next);
    }
    pages.remove_child_view(Some(&mut View::from(&view)));
    tracing::debug!(%id, empty, "tab view removed");
    if empty {
        // No tab left: the window goes too (like closing its last tab).
        tracing::info!(window = key, "no tab left after a close: window closes");
        window.close();
    }
}

// ------------------------------------------------------------------ UI -> browser

/// Execute a `bmcp://cmd/` command. UI thread. `sender` is the browser id that made the
/// request; anything that is not our UI/palette is ignored.
pub(crate) fn handle_command(sender: i32, command: Command) {
    let sender_view = with_ui(|ui| {
        ui.chrome_views
            .get(&sender)
            .map(|view| (view.kind, view.browser.clone()))
    });
    let Some((kind, sender_browser)) = sender_view else {
        tracing::warn!(
            sender,
            ?command,
            "command from a non-chrome browser ignored"
        );
        return;
    };
    let Some(key) = window_key(&sender_browser) else {
        return;
    };
    tracing::debug!(?kind, ?command, "chrome command");
    match command {
        Command::Layout(area) => {
            if kind != ViewKind::Ui {
                return; // only the chrome page decides where the page goes
            }
            let insets = Insets::from_area(area);
            let changed = with_ui(|ui| {
                let w = ui.windows.get_mut(&key)?;
                let changed = w.insets != insets;
                w.insets = insets;
                Some(changed)
            });
            if changed == Some(true) {
                place_views(key);
            }
        }
        Command::Palette(action) => set_palette(key, action),
        Command::FocusContent => focus_content(key),
        Command::FocusOmnibox => {
            if let Some(views) = views(key) {
                set_palette(key, PaletteAction::Hide);
                focus_omnibox(&views);
            }
        }
    }
}

/// Keyboard focus to the shown page.
fn focus_content(key: i32) {
    let active = with_ui(|ui| ui.windows.get(&key)?.strip.active());
    if let Some(view) = active.and_then(tab_view) {
        view.request_focus();
    }
}

/// Keyboard focus to the UI view, and its omnibox.
fn focus_omnibox(views: &Views) {
    views.ui.request_focus();
    run_js(&views.ui, &protocol::run_script("omnibox.focus"));
}

/// A navigation that tried to leave a chrome page (e.g. a link dropped on the UI)
/// is opened in the shown tab of the same window instead.
pub(crate) fn open_in_content(chrome_browser: &Browser, url: &str) {
    let Some(key) = window_key(chrome_browser) else {
        return;
    };
    let active = with_ui(|ui| {
        let id = ui.windows.get(&key)?.strip.active()?;
        ui.tabs.get(&id).map(|t| t.browser.clone())
    });
    if let Some(frame) = active.and_then(|b| b.main_frame()) {
        frame.load_url(Some(&CefString::from(url)));
    }
}

/// Handle a browser-wide shortcut pressed in a page or in the palette. Returns true if
/// it was consumed.
pub(crate) fn handle_shortcut(browser: &Browser, shortcut: Shortcut) -> bool {
    let Some(key) = window_key(browser) else {
        return false; // not a composed window (single layout)
    };
    let Some(views) = views(key) else {
        return false;
    };
    match shortcut {
        Shortcut::TogglePalette => set_palette(key, PaletteAction::Toggle),
        Shortcut::FocusOmnibox => {
            set_palette(key, PaletteAction::Hide);
            focus_omnibox(&views);
        }
        Shortcut::Escape => {
            if !palette_visible(&views) {
                return false; // let the page see its Esc
            }
            set_palette(key, PaletteAction::Hide);
        }
        Shortcut::Find => {
            set_palette(key, PaletteAction::Hide);
            // The find field must get the keys, so the UI view needs keyboard focus first.
            views.ui.request_focus();
            run_js(&views.ui, &protocol::run_script("find.open"));
        }
        Shortcut::Ui(command) => {
            set_palette(key, PaletteAction::Hide);
            // Same code path as pressing the key in the UI itself (its keymap + WS calls).
            run_js(&views.ui, &protocol::run_script(command));
        }
    }
    true
}

// ------------------------------------------------------------------ layout + palette

/// Place the UI view (whole window) and the pages panel (the UI's page slot).
fn place_views(key: i32) {
    let state = with_ui(|ui| {
        let w = ui.windows.get(&key)?;
        Some((w.views.clone(), w.insets, w.size))
    });
    let Some((views, insets, (width, height))) = state else {
        return;
    };
    if width <= 0 || height <= 0 {
        return; // not laid out yet
    }
    views.ui.set_bounds(Some(&Rect {
        x: 0,
        y: 0,
        width,
        height,
    }));
    let page = protocol::page_bounds(width, height, insets);
    views.pages.set_bounds(Some(&cef_rect(page)));
    size_hidden_tabs(key, &views);
    position_palette(&views, page);
}

fn palette_visible(views: &Views) -> bool {
    views
        .overlay
        .as_ref()
        .is_some_and(|overlay| overlay.is_visible() == 1)
}

fn set_palette(key: i32, action: PaletteAction) {
    let Some(views) = views(key) else { return };
    let Some(overlay) = &views.overlay else {
        return;
    };
    let visible = overlay.is_visible() == 1;
    let show = match action {
        PaletteAction::Show => true,
        PaletteAction::Hide => false,
        PaletteAction::Toggle => !visible,
    };
    if show && !visible {
        place_views(key);
        overlay.set_visible(1);
        views.palette.request_focus();
        run_js(&views.palette, "window.bmcp && window.bmcp.onShow();");
    } else if !show && visible {
        overlay.set_visible(0);
        run_js(&views.palette, "window.bmcp && window.bmcp.onHide();");
        // Focus goes back to the page, like closing a dialog.
        focus_content(key);
    }
}

/// Palette near the top of the page area, centred.
fn position_palette(views: &Views, page: Bounds) {
    if let Some(overlay) = &views.overlay {
        overlay.set_bounds(Some(&cef_rect(protocol::palette_bounds(page))));
    }
}

// ------------------------------------------------------------------ the window

wrap_window_delegate! {
    struct ChromeWindowDelegate {
        key: i32,
        parts: RefCell<Option<Parts>>,
        style: RuntimeStyle,
    }

    impl ViewDelegate {
        fn preferred_size(&self, _view: Option<&mut View>) -> Size {
            Size { width: 1280, height: 800 }
        }

        fn on_layout_changed(&self, _view: Option<&mut View>, new_bounds: Option<&Rect>) {
            // Window resized (or DPI changed): keep UI, page and palette in place.
            let Some(bounds) = new_bounds else { return };
            let size = (bounds.width, bounds.height);
            let known = with_ui(|ui| {
                ui.windows.get_mut(&self.key).map(|w| w.size = size).is_some()
            });
            if known {
                place_views(self.key);
            }
        }
    }

    impl PanelDelegate {}

    impl WindowDelegate {
        fn on_window_created(&self, window: Option<&mut Window>) {
            let parts = self.parts.borrow_mut().take();
            let (Some(window), Some(parts)) = (window, parts) else {
                return;
            };
            window.set_id(self.key);
            window.set_title(Some(&CefString::from("BrowserMCP")));

            // Register the window BEFORE adding views: adding the first tab can create
            // its browser synchronously, and `register_tab` must find the window.
            let views = Views {
                window: window.clone(),
                ui: parts.ui.clone(),
                pages: parts.pages.clone(),
                palette: parts.palette.clone(),
                overlay: None,
            };
            with_ui(|ui| {
                ui.windows.insert(
                    self.key,
                    ChromeWindow {
                        views,
                        strip: TabStrip::new(),
                        insets: Insets::DEFAULT,
                        size: (0, 0),
                        closing: false,
                    },
                );
                ui.last_window = Some(self.key);
            });

            // No layout manager: `place_views` positions both children. Children added
            // later are drawn on top, so the pages cover the UI's (transparent) slot.
            window.add_child_view(Some(&mut View::from(&parts.ui)));
            window.add_child_view(Some(&mut View::from(&parts.pages)));

            // Overlays must be added after the other children to be on top. Hidden
            // until Ctrl+K. can_activate = 1 so the palette can take keyboard focus.
            let mut palette_view = View::from(&parts.palette);
            let overlay = window.add_overlay_view(Some(&mut palette_view), DockingMode::CUSTOM, 1);
            if overlay.is_none() {
                tracing::error!("add_overlay_view failed; palette disabled");
            }
            if let Some(overlay) = &overlay {
                overlay.set_visible(0);
            }
            with_ui(|ui| {
                if let Some(w) = ui.windows.get_mut(&self.key) {
                    w.views.overlay = overlay;
                }
            });

            parts.pages.add_child_view(Some(&mut View::from(&parts.first_tab)));
            window.show();
        }

        fn on_window_destroyed(&self, _window: Option<&mut Window>) {
            self.parts.borrow_mut().take();
            let removed = with_ui(|ui| {
                if ui.last_window == Some(self.key) {
                    ui.last_window = None;
                }
                ui.windows.remove(&self.key)
            });
            drop(removed); // released outside the UiState borrow
        }

        fn on_window_activation_changed(&self, _window: Option<&mut Window>, active: i32) {
            if active == 1 {
                with_ui(|ui| ui.last_window = Some(self.key));
            }
        }

        fn can_close(&self, _window: Option<&mut Window>) -> i32 {
            // Only the pages vote (each may run beforeunload and cancel). Pitfall: do NOT
            // call try_close_browser() on our UI/palette browsers: each call can re-enter
            // can_close synchronously, the votes interleave and the window never closes.
            // They are closed together with the window.
            let browsers = with_ui(|ui| {
                let w = ui.windows.get_mut(&self.key)?;
                w.closing = true;
                let ids = w.strip.tabs().to_vec();
                Some(
                    ids.iter()
                        .filter_map(|id| ui.tabs.get(id).map(|t| t.browser.clone()))
                        .collect::<Vec<_>>(),
                )
            });
            let Some(browsers) = browsers else { return 1 };
            // Who closes windows: the close button / Alt+F4 (a person), `quit`, or the last
            // tab going away (logged in do_close / detach_tab).
            tracing::info!(window = self.key, tabs = browsers.len(), "window close requested");
            let mut all = true;
            for browser in browsers {
                // Returns 1 when that page is ready; otherwise it starts closing and
                // its DoClose sends the window another close request later.
                if browser.host().map_or(1, |host| host.try_close_browser()) != 1 {
                    all = false;
                }
            }
            tracing::debug!(all, "can_close");
            if all {
                // Like cefclient's overlay browser: destroy the overlay explicitly
                // before the window goes away.
                let overlay = with_ui(|ui| ui.windows.get_mut(&self.key)?.views.overlay.take());
                if let Some(overlay) = overlay {
                    overlay.destroy();
                }
            }
            i32::from(all)
        }

        // Views windows default to "close button only"; a browser wants all three.
        fn can_resize(&self, _window: Option<&mut Window>) -> i32 {
            1
        }

        fn can_maximize(&self, _window: Option<&mut Window>) -> i32 {
            1
        }

        fn can_minimize(&self, _window: Option<&mut Window>) -> i32 {
            1
        }

        fn window_runtime_style(&self) -> RuntimeStyle {
            self.style
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{KeepWindowPlan, keep_window_plan};

    #[test]
    fn automation_never_closes_the_last_window() {
        // Last tab of a composed window: the new tab page comes first.
        assert_eq!(
            keep_window_plan(Some((7, 1)), false),
            KeepWindowPlan::ReplaceThenClose(7)
        );
        // Other tabs remain: close normally.
        assert_eq!(keep_window_plan(Some((7, 3)), false), KeepWindowPlan::Close);
        // Off-screen tabs have no window to keep.
        assert_eq!(keep_window_plan(None, true), KeepWindowPlan::Close);
        // Single layout: the tab is the window, so it shows the new tab page instead.
        assert_eq!(
            keep_window_plan(None, false),
            KeepWindowPlan::ShowNewTabInstead
        );
    }
}
