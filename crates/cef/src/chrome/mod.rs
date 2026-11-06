//! Spike S1: a browser window composed of several CEF Views (Alloy style).
//!
//! ```text
//! +-------------------------------------------+
//! | UI view  (bmcp://ui/index.html, 80 DIP)    |  <- our chrome: buttons, URL box
//! +-------------------------------------------+
//! |                                           |
//! | content view (the web page, flex = 1)     |  <- the only browser that is a "tab"
//! |        +---------------------+            |
//! |        | palette overlay     |            |  <- Window::add_overlay_view, hidden
//! |        | (bmcp://ui/palette) |            |     until Ctrl+K
//! |        +---------------------+            |
//! +-------------------------------------------+
//! ```
//!
//! * The window uses a vertical `BoxLayout`; sizes are in DIP, so DPI scaling is free.
//! * Each window gets a numeric key stored as the Window's view id
//!   (`Window::set_id`). Any handler can find "its" window from a `Browser` with
//!   [`window_key`], which also works for popups (they inherit the parent's client).
//! * The views of every window live in `UiState::windows`; UI/palette browsers are in
//!   `UiState::chrome_views` and are never registered as tabs (not visible over MCP).
//! * Command channel UI → browser and browser → UI: see [`protocol`].

pub(crate) mod protocol;

use std::cell::RefCell;
use std::sync::atomic::{AtomicI32, Ordering};

use bmcp_core::TabId;
use cef::*;

use crate::bridge::{ShellContext, with_ui};
use crate::scheme::{PALETTE_URL, UI_URL};
use protocol::{Bounds, Command, ContentState, PaletteAction, Shortcut};

/// Height of the UI strip in DIP (scaled by the OS DPI automatically).
pub(crate) const UI_HEIGHT: i32 = 80;

/// What a browser is used for. Decides which handlers apply and whether it is a tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ViewKind {
    /// A web page. Registered as a tab; visible to MCP.
    Content,
    /// Our chrome strip at the top of the window.
    Ui,
    /// The Ctrl+K palette overlay.
    Palette,
}

/// A UI or palette browser (never a tab).
pub(crate) struct ChromeView {
    pub kind: ViewKind,
    pub browser: Browser,
}

/// The views of one composed window. Cheap to clone (CEF references).
#[derive(Clone)]
pub(crate) struct ChromeWindow {
    pub ui: BrowserView,
    pub content: BrowserView,
    pub palette: BrowserView,
    pub overlay: Option<OverlayController>,
}

/// Views created before the window exists; moved into the window in `on_window_created`.
#[derive(Clone)]
struct Parts {
    ui: BrowserView,
    content: BrowserView,
    palette: BrowserView,
}

/// Window keys start at 1 (0 is the default view id).
fn next_key() -> i32 {
    static NEXT: AtomicI32 = AtomicI32::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// Open a composed window showing `url`.
pub(crate) fn open_window(url: &str, ctx: &ShellContext) {
    let content = crate::shell::create_browser_view(url, ViewKind::Content, ctx);
    if let Some(content) = content {
        open_window_with(content, ctx);
    }
}

/// Open a composed window around an existing content view (used for popups).
pub(crate) fn open_window_with(content: BrowserView, ctx: &ShellContext) {
    let ui = crate::shell::create_browser_view(UI_URL, ViewKind::Ui, ctx);
    let palette = crate::shell::create_browser_view(PALETTE_URL, ViewKind::Palette, ctx);
    let (Some(ui), Some(palette)) = (ui, palette) else {
        tracing::error!("could not create the chrome views");
        return;
    };
    let parts = Parts {
        ui,
        content,
        palette,
    };
    let mut delegate = ChromeWindowDelegate::new(next_key(), RefCell::new(Some(parts)), ctx.style);
    window_create_top_level(Some(&mut delegate));
}

/// Key of the composed window that hosts `browser`, if any.
pub(crate) fn window_key(browser: &Browser) -> Option<i32> {
    let mut browser = browser.clone();
    browser_view_get_for_browser(Some(&mut browser))
        .and_then(|view| view.window())
        .map(|window| window.id())
        .filter(|id| *id != 0)
}

/// Copy of the views of a window (the UiState borrow ends before we call CEF).
fn chrome_window(key: i32) -> Option<ChromeWindow> {
    with_ui(|ui| ui.windows.get(&key).cloned())
}

fn window_of(browser: &Browser) -> Option<ChromeWindow> {
    window_key(browser).and_then(chrome_window)
}

fn run_js(view: &BrowserView, code: &str) {
    if let Some(frame) = view.browser().and_then(|b| b.main_frame()) {
        frame.execute_java_script(Some(&CefString::from(code)), None, 0);
    }
}

// ------------------------------------------------------------------ browser -> UI

/// Push the content state (url/title/loading/history) to the UI strip of the window
/// that hosts `content`. No-op outside composed windows.
pub(crate) fn push_state(content: &Browser) {
    let Some(window) = window_of(content) else {
        return;
    };
    push_window_state(&window);
}

fn push_window_state(window: &ChromeWindow) {
    let Some(content) = window.content.browser() else {
        return;
    };
    let id = TabId(content.identifier());
    let tab = with_ui(|ui| {
        ui.tabs
            .get(&id)
            .map(|t| (t.url.clone(), t.title.clone(), t.load.borrow().loading))
    });
    let (url, title, loading) = tab.unwrap_or_default();
    let state = ContentState {
        url,
        title,
        loading,
        can_go_back: content.can_go_back() == 1,
        can_go_forward: content.can_go_forward() == 1,
    };
    run_js(&window.ui, &protocol::state_script(&state));
}

// ------------------------------------------------------------------ UI -> browser

/// Execute a command from a chrome page. UI thread. `sender` is the browser id that
/// made the `bmcp://cmd/` request; anything that is not our UI/palette is ignored.
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
    let Some(window) = window_of(&sender_browser) else {
        return;
    };
    tracing::debug!(?kind, ?command, "chrome command");
    let content = window.content.browser();
    match command {
        Command::Ready => push_window_state(&window),
        Command::Navigate(text) => {
            if let (Some(url), Some(frame)) = (
                protocol::fixup_url(&text),
                content.as_ref().and_then(|b| b.main_frame()),
            ) {
                frame.load_url(Some(&CefString::from(url.as_str())));
                set_palette(&window, PaletteAction::Hide);
                window.content.request_focus();
            }
        }
        Command::Back => history(content, Browser::go_back),
        Command::Forward => history(content, Browser::go_forward),
        Command::Reload => history(content, Browser::reload),
        Command::Stop => history(content, Browser::stop_load),
        Command::Palette(action) => set_palette(&window, action),
        Command::FocusContent => window.content.request_focus(),
    }
}

/// Run a simple browser action (back, reload, ...) on the content browser if it exists.
fn history(content: Option<Browser>, action: fn(&Browser)) {
    if let Some(browser) = content {
        action(&browser);
    }
}

/// A navigation that tried to leave a chrome page (e.g. a link dropped on the UI strip)
/// is redirected to the content view of the same window.
pub(crate) fn open_in_content(chrome_browser: &Browser, url: &str) {
    let Some(window) = window_of(chrome_browser) else {
        return;
    };
    if let Some(frame) = window.content.browser().and_then(|b| b.main_frame()) {
        frame.load_url(Some(&CefString::from(url)));
    }
}

/// Handle a browser-wide shortcut. Returns true if it was consumed.
pub(crate) fn handle_shortcut(browser: &Browser, shortcut: Shortcut) -> bool {
    let Some(window) = window_of(browser) else {
        return false; // not a composed window (single layout)
    };
    let content = window.content.browser();
    match shortcut {
        Shortcut::TogglePalette => set_palette(&window, PaletteAction::Toggle),
        Shortcut::FocusOmnibox => {
            set_palette(&window, PaletteAction::Hide);
            window.ui.request_focus();
            run_js(&window.ui, "window.bmcp && window.bmcp.focusUrl();");
        }
        Shortcut::Reload => history(content, Browser::reload),
        Shortcut::Back => history(content, Browser::go_back),
        Shortcut::Forward => history(content, Browser::go_forward),
        Shortcut::Escape => {
            if !palette_visible(&window) {
                return false; // let the page see its Esc
            }
            set_palette(&window, PaletteAction::Hide);
        }
    }
    true
}

// ------------------------------------------------------------------ palette overlay

fn palette_visible(window: &ChromeWindow) -> bool {
    window
        .overlay
        .as_ref()
        .is_some_and(|overlay| overlay.is_visible() == 1)
}

fn set_palette(window: &ChromeWindow, action: PaletteAction) {
    let Some(overlay) = &window.overlay else {
        return;
    };
    let visible = overlay.is_visible() == 1;
    let show = match action {
        PaletteAction::Show => true,
        PaletteAction::Hide => false,
        PaletteAction::Toggle => !visible,
    };
    if show && !visible {
        position_palette(window);
        overlay.set_visible(1);
        window.palette.request_focus();
        run_js(&window.palette, "window.bmcp && window.bmcp.onShow();");
    } else if !show && visible {
        overlay.set_visible(0);
        // Focus goes back to the page, like closing a dialog.
        window.content.request_focus();
    }
}

/// Centre the palette over the content view. Called on show and on every relayout.
fn position_palette(window: &ChromeWindow) {
    let Some(overlay) = &window.overlay else {
        return;
    };
    let r = window.content.bounds();
    let b = protocol::palette_bounds(Bounds {
        x: r.x,
        y: r.y,
        width: r.width,
        height: r.height,
    });
    overlay.set_bounds(Some(&Rect {
        x: b.x,
        y: b.y,
        width: b.width,
        height: b.height,
    }));
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

        fn on_layout_changed(&self, _view: Option<&mut View>, _new_bounds: Option<&Rect>) {
            // Window resized (or DPI changed): keep the overlay centred.
            if let Some(window) = chrome_window(self.key) {
                position_palette(&window);
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

            // UI strip on top (its height comes from the view delegate's preferred
            // size), the page takes all remaining space (flex 1), both full width.
            let settings = BoxLayoutSettings {
                horizontal: 0,
                cross_axis_alignment: AxisAlignment::STRETCH,
                ..Default::default()
            };
            let layout = window.set_to_box_layout(Some(&settings));
            let mut ui_view = View::from(&parts.ui);
            let mut content_view = View::from(&parts.content);
            window.add_child_view(Some(&mut ui_view));
            window.add_child_view(Some(&mut content_view));
            if let Some(layout) = layout {
                layout.set_flex_for_view(Some(&mut content_view), 1);
            }

            // Overlays must be added after the other children to be on top. Hidden
            // until Ctrl+K. can_activate = 1 so the palette can take keyboard focus.
            let mut palette_view = View::from(&parts.palette);
            let overlay = window.add_overlay_view(Some(&mut palette_view), DockingMode::CUSTOM, 1);
            if overlay.is_none() {
                tracing::error!("add_overlay_view failed; palette disabled");
            }

            let entry = ChromeWindow {
                ui: parts.ui,
                content: parts.content.clone(),
                palette: parts.palette,
                overlay,
            };
            with_ui(|ui| ui.windows.insert(self.key, entry));
            window.show();
            parts.content.request_focus();
        }

        fn on_window_destroyed(&self, _window: Option<&mut Window>) {
            self.parts.borrow_mut().take();
            let removed = with_ui(|ui| ui.windows.remove(&self.key));
            drop(removed); // released outside the UiState borrow
        }

        fn can_close(&self, _window: Option<&mut Window>) -> i32 {
            let Some(window) = chrome_window(self.key) else {
                return 1;
            };
            // Only the page decides (it may run beforeunload and cancel). Pitfall: do NOT
            // call try_close_browser() on our UI/palette browsers too: each call can
            // re-enter can_close synchronously, the votes interleave and the window
            // never closes (process hang). They are closed together with the window.
            let content = window
                .content
                .browser()
                .and_then(|b| b.host())
                .map_or(1, |host| host.try_close_browser());
            tracing::debug!(content, "can_close");
            if content == 1 {
                // Like cefclient's overlay browser: destroy the overlay explicitly
                // before the window goes away.
                let overlay = with_ui(|ui| ui.windows.get_mut(&self.key)?.overlay.take());
                if let Some(overlay) = overlay {
                    overlay.destroy();
                }
            }
            content
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
