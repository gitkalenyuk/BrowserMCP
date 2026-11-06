//! Browser shell: process handler, per-browser client and handlers, windows.
//!
//! Two window layouts (see [`crate::WindowLayout`]):
//! * `Chrome` (default): the Svelte UI around the tabs' pages + Ctrl+K overlay, see
//!   [`crate::chrome`].
//! * `Single` (Phase 0): one BrowserView per top-level window, title follows the page.
//!
//! The app quits when the last browser (of any kind) closes.

use std::cell::RefCell;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use bmcp_core::{BackendError, BrowserBackend, TabId};
use cef::*;
use tokio::sync::oneshot;

use crate::bridge::{self, LoadState, PendingTab, ShellContext};
use crate::chrome::{self, ViewKind, protocol};
use crate::{Style, WindowLayout};

/// Called once on the UI thread when CEF is ready and the first window exists.
pub type ReadyCallback = Box<dyn FnOnce(Arc<dyn BrowserBackend>) + Send>;

pub struct StartConfig {
    pub url: String,
    pub style: Style,
    pub layout: WindowLayout,
    pub on_ready: Mutex<Option<ReadyCallback>>,
}

fn runtime_style(style: Style) -> RuntimeStyle {
    match style {
        Style::Chrome => RuntimeStyle::CHROME,
        Style::Alloy => RuntimeStyle::ALLOY,
    }
}

/// Number of live browsers; when it drops to zero the message loop quits.
#[derive(Default)]
pub(crate) struct Browsers {
    open: AtomicUsize,
}

wrap_app! {
    pub struct ShellApp {
        config: Arc<StartConfig>,
    }

    impl App {
        fn on_before_command_line_processing(
            &self,
            process_type: Option<&CefString>,
            command_line: Option<&mut CommandLine>,
        ) {
            crate::switches::apply(process_type, command_line);
        }

        fn on_register_custom_schemes(&self, registrar: Option<&mut SchemeRegistrar>) {
            crate::scheme::register_custom_schemes(registrar);
        }

        fn browser_process_handler(&self) -> Option<BrowserProcessHandler> {
            Some(ShellProcessHandler::new(self.config.clone()))
        }
    }
}

wrap_browser_process_handler! {
    struct ShellProcessHandler {
        config: Arc<StartConfig>,
    }

    impl BrowserProcessHandler {
        fn on_context_initialized(&self) {
            debug_assert_ne!(currently_on(ThreadId::UI), 0);
            crate::scheme::register_handler();
            let ctx = ShellContext {
                style: runtime_style(self.config.style),
                layout: self.config.layout,
                browsers: Arc::new(Browsers::default()),
            };
            bridge::with_ui(|ui| ui.shell = Some(ctx.clone()));
            open_window(&self.config.url, &ctx, None);

            let ready = self.config.on_ready.lock().unwrap_or_else(|e| e.into_inner()).take();
            if let Some(ready) = ready {
                ready(Arc::new(crate::backend::CefBackend));
            }
        }
    }
}

/// Open a new top-level window showing `url`, in the configured layout. `reply` learns
/// the id of its (first) tab.
pub(crate) fn open_window(url: &str, ctx: &ShellContext, reply: Option<oneshot::Sender<TabId>>) {
    match ctx.layout {
        WindowLayout::Chrome => chrome::open_window(url, ctx, reply),
        WindowLayout::Single => {
            if let Some(view) = create_browser_view(url, ViewKind::Content, ctx) {
                let view_id = bridge::next_view_id();
                view.set_id(view_id);
                let pending = PendingTab {
                    window: None,
                    index: None,
                    activate: true,
                    url: url.to_owned(),
                    reply,
                };
                bridge::with_ui(|ui| ui.pending.insert(view_id, pending));
                open_single_window(view, ctx.style);
            }
        }
    }
}

/// `tabs.open`: a tab in the current window (composed layout) or a new window (single).
pub(crate) fn open_tab(
    url: String,
    background: bool,
    reply: oneshot::Sender<TabId>,
) -> Result<(), BackendError> {
    let ctx = bridge::with_ui(|ui| ui.shell.clone()).ok_or(BackendError::ShuttingDown)?;
    match ctx.layout {
        WindowLayout::Chrome => chrome::open_tab(url, background, reply),
        WindowLayout::Single => {
            open_window(&url, &ctx, Some(reply));
            Ok(())
        }
    }
}

/// Create a BrowserView of the given kind. The browser itself is created by CEF once
/// the view is added to a window.
pub(crate) fn create_browser_view(
    url: &str,
    kind: ViewKind,
    ctx: &ShellContext,
) -> Option<BrowserView> {
    let mut client = ShellClient::new(ctx.browsers.clone(), kind);
    let mut view_delegate = ShellBrowserViewDelegate::new(ctx.clone(), kind);
    let mut settings = BrowserSettings::default();
    if kind != ViewKind::Content {
        // Dark background so our chrome does not flash white before its page paints.
        settings.background_color = 0xFF20_2124;
    }
    browser_view_create(
        Some(&mut client),
        Some(&CefString::from(url)),
        Some(&settings),
        None,
        None,
        Some(&mut view_delegate),
    )
}

/// Phase 0 layout: the BrowserView fills a window of its own.
fn open_single_window(browser_view: BrowserView, style: RuntimeStyle) {
    let mut window_delegate = SingleWindowDelegate::new(RefCell::new(Some(browser_view)), style);
    window_create_top_level(Some(&mut window_delegate));
}

wrap_window_delegate! {
    struct SingleWindowDelegate {
        browser_view: RefCell<Option<BrowserView>>,
        style: RuntimeStyle,
    }

    impl ViewDelegate {
        fn preferred_size(&self, _view: Option<&mut View>) -> Size {
            Size { width: 1280, height: 800 }
        }
    }

    impl PanelDelegate {}

    impl WindowDelegate {
        fn on_window_created(&self, window: Option<&mut Window>) {
            let browser_view = self.browser_view.borrow();
            let (Some(window), Some(browser_view)) = (window, browser_view.as_ref()) else {
                return;
            };
            window.set_title(Some(&CefString::from("BrowserMCP")));
            let mut view = View::from(browser_view);
            window.add_child_view(Some(&mut view));
            window.show();
        }

        fn on_window_destroyed(&self, _window: Option<&mut Window>) {
            self.browser_view.borrow_mut().take();
        }

        fn can_close(&self, _window: Option<&mut Window>) -> i32 {
            // Let the page run its beforeunload handlers first.
            let browser_view = self.browser_view.borrow();
            match browser_view.as_ref().and_then(|v| v.browser()).and_then(|b| b.host()) {
                Some(host) => host.try_close_browser(),
                None => 1,
            }
        }

        // Views windows default to "close button only".
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

wrap_browser_view_delegate! {
    struct ShellBrowserViewDelegate {
        ctx: ShellContext,
        kind: ViewKind,
    }

    impl ViewDelegate {
        fn preferred_size(&self, _view: Option<&mut View>) -> Size {
            // Pitfall: a size with a 0 width OR height counts as "empty" and CEF falls
            // back to the BrowserView default. UI and pages are placed explicitly by
            // `chrome::place_views`; only the palette size matters here.
            match self.kind {
                ViewKind::Palette => {
                    let (width, height) = protocol::PALETTE_SIZE;
                    Size { width, height }
                }
                ViewKind::Ui | ViewKind::Content => Size { width: 1, height: 1 },
            }
        }
    }

    impl BrowserViewDelegate {
        fn delegate_for_popup_browser_view(
            &self,
            _browser_view: Option<&mut BrowserView>,
            _settings: Option<&BrowserSettings>,
            _client: Option<&mut Client>,
            _is_devtools: i32,
        ) -> Option<BrowserViewDelegate> {
            // Popups are web pages, whatever opened them.
            Some(ShellBrowserViewDelegate::new(self.ctx.clone(), ViewKind::Content))
        }

        fn on_popup_browser_view_created(
            &self,
            browser_view: Option<&mut BrowserView>,
            popup_browser_view: Option<&mut BrowserView>,
            is_devtools: i32,
        ) -> i32 {
            let Some(popup) = popup_browser_view.cloned() else { return 0 };
            let opener = browser_view.and_then(|v| v.browser());
            // Phase 2: a page's popup becomes a tab of the same window.
            let adopted = self.ctx.layout == WindowLayout::Chrome
                && is_devtools == 0
                && opener.is_some_and(|opener| chrome::adopt_popup(&opener, &popup));
            if !adopted {
                open_single_window(popup, self.ctx.style);
            }
            1
        }

        fn browser_runtime_style(&self) -> RuntimeStyle {
            match self.kind {
                // CEF: an overlay BrowserView must always be Alloy style.
                ViewKind::Palette => RuntimeStyle::ALLOY,
                _ => self.ctx.style,
            }
        }
    }
}

wrap_client! {
    struct ShellClient {
        browsers: Arc<Browsers>,
        kind: ViewKind,
    }

    impl Client {
        fn display_handler(&self) -> Option<DisplayHandler> {
            Some(ShellDisplayHandler::new(self.kind))
        }

        fn life_span_handler(&self) -> Option<LifeSpanHandler> {
            Some(ShellLifeSpanHandler::new(self.browsers.clone(), self.kind))
        }

        fn load_handler(&self) -> Option<LoadHandler> {
            Some(ShellLoadHandler::new(self.kind))
        }

        fn keyboard_handler(&self) -> Option<KeyboardHandler> {
            Some(ShellKeyboardHandler::new(self.kind))
        }

        fn request_handler(&self) -> Option<RequestHandler> {
            Some(ShellRequestHandler::new(self.kind))
        }

        fn context_menu_handler(&self) -> Option<ContextMenuHandler> {
            Some(ShellContextMenuHandler::new(self.kind))
        }

        fn permission_handler(&self) -> Option<PermissionHandler> {
            Some(ShellPermissionHandler::new())
        }
    }
}

wrap_permission_handler! {
    struct ShellPermissionHandler;

    impl PermissionHandler {
        /// Alloy shows no permission UI, and an unanswered prompt waits forever — e.g.
        /// every connection from a page to 127.0.0.1 (Local Network Access) hung, so the
        /// UI could never reach the API. Answer at once: see `scheme::allow_permission`.
        fn on_show_permission_prompt(
            &self,
            _browser: Option<&mut Browser>,
            _prompt_id: u64,
            requesting_origin: Option<&CefString>,
            requested_permissions: u32,
            callback: Option<&mut PermissionPromptCallback>,
        ) -> i32 {
            let Some(callback) = callback else { return 0 };
            let origin = requesting_origin.map(CefString::to_string).unwrap_or_default();
            let allow = crate::scheme::allow_permission(&origin, requested_permissions);
            tracing::debug!(origin, requested_permissions, allow, "permission prompt");
            callback.cont(if allow {
                PermissionRequestResult::ACCEPT
            } else {
                PermissionRequestResult::DENY
            });
            1
        }
    }
}

/// Scheme + host + port of a URL (`https://a.test:8080`), to notice a new site.
fn origin_of(url: &str) -> &str {
    match url.find("://") {
        Some(i) => {
            let rest = &url[i + 3..];
            let end = rest.find(['/', '?', '#']).map_or(url.len(), |e| i + 3 + e);
            &url[..end]
        }
        None => url,
    }
}

wrap_display_handler! {
    struct ShellDisplayHandler {
        kind: ViewKind,
    }

    impl DisplayHandler {
        fn on_address_change(
            &self,
            browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            url: Option<&CefString>,
        ) {
            let (Some(browser), Some(frame)) = (browser, frame) else { return };
            if self.kind == ViewKind::Content && frame.is_main() == 1 {
                let url = url.map(CefString::to_string).unwrap_or_default();
                bridge::update_tab(browser, |tab| {
                    // A new site: its favicon is unknown until it reports one.
                    if origin_of(&tab.url) != origin_of(&url) {
                        tab.favicon = None;
                    }
                    tab.url = url;
                    true
                });
            }
        }

        fn on_title_change(&self, browser: Option<&mut Browser>, title: Option<&CefString>) {
            if self.kind != ViewKind::Content {
                return; // our own pages must not rename the window
            }
            let Some(browser) = browser else { return };
            let title = title.map(CefString::to_string).unwrap_or_default();
            bridge::update_tab(browser, |tab| {
                tab.title = title;
                true
            });
            let id = TabId(browser.identifier());
            chrome::tab_title_changed(id);
            // Single layout: the window shows just this page.
            let is_single = bridge::with_ui(|ui| ui.tabs.get(&id).is_some_and(|t| t.window.is_none()));
            if is_single {
                let mut b = browser.clone();
                if let Some(window) = browser_view_get_for_browser(Some(&mut b)).and_then(|v| v.window()) {
                    let title = bridge::tab_info(id).map(|t| t.title).unwrap_or_default();
                    let title = if title.is_empty() { "BrowserMCP".to_owned() } else { format!("{title} — BrowserMCP") };
                    window.set_title(Some(&CefString::from(title.as_str())));
                }
            }
        }

        /// Console output of our own UI/palette pages goes to our log (web pages' does not).
        fn on_console_message(
            &self,
            _browser: Option<&mut Browser>,
            level: LogSeverity,
            message: Option<&CefString>,
            source: Option<&CefString>,
            line: ::std::os::raw::c_int,
        ) -> ::std::os::raw::c_int {
            if self.kind != ViewKind::Content {
                let message = message.map(CefString::to_string).unwrap_or_default();
                let source = source.map(CefString::to_string).unwrap_or_default();
                let kind = self.kind;
                if level == LogSeverity::ERROR || level == LogSeverity::WARNING {
                    tracing::warn!(?kind, source, line, "console: {message}");
                } else {
                    tracing::debug!(?kind, source, line, "console: {message}");
                }
            }
            0
        }

        fn on_favicon_urlchange(
            &self,
            browser: Option<&mut Browser>,
            icon_urls: Option<&mut CefStringList>,
        ) {
            if self.kind != ViewKind::Content {
                return;
            }
            let (Some(browser), Some(icon_urls)) = (browser, icon_urls) else { return };
            // The page's candidates, best first; we take the first one. Pitfall: cef-rs'
            // `CefStringList::clone()` makes a copy that iterates as empty, so move the
            // borrowed list out (leaving an empty list of our own behind, freed on drop).
            let list = std::mem::take(icon_urls);
            let first = list.into_iter().next();
            bridge::update_tab(browser, |tab| {
                let changed = tab.favicon != first;
                tab.favicon = first;
                changed
            });
        }
    }
}

wrap_life_span_handler! {
    struct ShellLifeSpanHandler {
        browsers: Arc<Browsers>,
        kind: ViewKind,
    }

    impl LifeSpanHandler {
        fn on_before_popup(
            &self,
            browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _popup_id: i32,
            target_url: Option<&CefString>,
            _target_frame_name: Option<&CefString>,
            target_disposition: WindowOpenDisposition,
            _user_gesture: i32,
            _popup_features: Option<&PopupFeatures>,
            _window_info: Option<&mut WindowInfo>,
            _client: Option<&mut Option<Client>>,
            _settings: Option<&mut BrowserSettings>,
            _extra_info: Option<&mut Option<DictionaryValue>>,
            _no_javascript_access: Option<&mut i32>,
        ) -> i32 {
            let url = target_url.map(CefString::to_string).unwrap_or_default();
            if self.kind == ViewKind::Content {
                // Allowed; it becomes a tab in on_popup_browser_view_created. Remember
                // whether it should open in the background (Ctrl+click, middle click).
                if let Some(opener) = browser {
                    let background = target_disposition == WindowOpenDisposition::NEW_BACKGROUND_TAB;
                    bridge::with_ui(|ui| ui.popups.insert(opener.identifier(), (background, url)));
                }
                return 0;
            }
            // Our chrome pages never open windows.
            tracing::warn!(url, "popup from a chrome view blocked");
            1
        }

        fn on_after_created(&self, browser: Option<&mut Browser>) {
            if let Some(browser) = browser {
                match self.kind {
                    ViewKind::Content => bridge::register_tab(browser),
                    kind => bridge::register_chrome_view(browser, kind),
                }
            }
            let open = self.browsers.open.fetch_add(1, Ordering::SeqCst) + 1;
            tracing::debug!(open, kind = ?self.kind, "browser created");
        }

        fn do_close(&self, browser: Option<&mut Browser>) -> i32 {
            match (self.kind, browser) {
                (ViewKind::Content, Some(browser)) => chrome::do_close(browser),
                _ => 0,
            }
        }

        fn on_before_close(&self, browser: Option<&mut Browser>) {
            if let Some(browser) = browser {
                match self.kind {
                    ViewKind::Content => bridge::unregister_tab(browser),
                    _ => bridge::unregister_chrome_view(browser),
                }
            }
            let open = self.browsers.open.fetch_sub(1, Ordering::SeqCst) - 1;
            tracing::debug!(open, kind = ?self.kind, "browser closed");
            if open == 0 {
                quit_message_loop();
            }
        }
    }
}

wrap_load_handler! {
    struct ShellLoadHandler {
        kind: ViewKind,
    }

    impl LoadHandler {
        fn on_loading_state_change(
            &self,
            browser: Option<&mut Browser>,
            is_loading: ::std::os::raw::c_int,
            can_go_back: ::std::os::raw::c_int,
            can_go_forward: ::std::os::raw::c_int,
        ) {
            let Some(browser) = browser else { return };
            if self.kind != ViewKind::Content {
                return;
            }
            let loading = is_loading == 1;
            bridge::update_tab(browser, |tab| {
                tab.load.send_modify(|s: &mut LoadState| {
                    if s.loading && !loading {
                        s.seq += 1;
                    }
                    s.loading = loading;
                });
                tab.can_go_back = can_go_back == 1;
                tab.can_go_forward = can_go_forward == 1;
                true
            });
        }
    }
}

wrap_keyboard_handler! {
    struct ShellKeyboardHandler {
        kind: ViewKind,
    }

    impl KeyboardHandler {
        /// Runs before the page sees the key. Pages and the palette forward browser
        /// shortcuts; the UI view handles its own keys (its keymap sees them anyway).
        fn on_pre_key_event(
            &self,
            browser: Option<&mut Browser>,
            event: Option<&KeyEvent>,
            _os_event: Option<&mut sys::MSG>,
            _is_keyboard_shortcut: Option<&mut i32>,
        ) -> i32 {
            let (Some(browser), Some(event)) = (browser, event) else { return 0 };
            if self.kind == ViewKind::Ui || event.type_ != KeyEventType::RAWKEYDOWN {
                return 0;
            }
            let flag = |f: sys::cef_event_flags_t| event.modifiers & f.0 as u32 != 0;
            let modifiers = protocol::Modifiers {
                ctrl: flag(sys::cef_event_flags_t::EVENTFLAG_CONTROL_DOWN),
                alt: flag(sys::cef_event_flags_t::EVENTFLAG_ALT_DOWN),
                shift: flag(sys::cef_event_flags_t::EVENTFLAG_SHIFT_DOWN),
            };
            match protocol::shortcut(event.windows_key_code, modifiers) {
                Some(shortcut) => i32::from(chrome::handle_shortcut(browser, shortcut)),
                None => 0,
            }
        }
    }
}

wrap_request_handler! {
    struct ShellRequestHandler {
        kind: ViewKind,
    }

    impl RequestHandler {
        fn on_before_browse(
            &self,
            browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            request: Option<&mut Request>,
            _user_gesture: i32,
            _is_redirect: i32,
        ) -> i32 {
            let (Some(browser), Some(frame), Some(request)) = (browser, frame, request) else {
                return 0;
            };
            let url = CefString::from(&request.url()).to_string();
            if self.kind == ViewKind::Content {
                // A tab may show our new tab page, but never the chrome/palette pages
                // (they have full API access) nor send bmcp://cmd navigations.
                if crate::scheme::is_allowed_in_content(&url) {
                    return 0;
                }
                tracing::warn!(url, "blocked bmcp:// page in a tab");
                return 1;
            }
            if frame.is_main() == 0 || crate::scheme::is_chrome_url(&url) {
                return 0;
            }
            // Our chrome pages stay put. A link dropped on the UI (or any other attempt
            // to leave bmcp://ui/) opens in the shown tab instead.
            chrome::open_in_content(browser, &url);
            1
        }
    }
}

wrap_context_menu_handler! {
    struct ShellContextMenuHandler {
        kind: ViewKind,
    }

    impl ContextMenuHandler {
        fn on_before_context_menu(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            params: Option<&mut ContextMenuParams>,
            model: Option<&mut MenuModel>,
        ) {
            // Alloy's default menu (Back, Reload, View source, ...) makes no sense on our
            // chrome; keep it only for text fields (cut/copy/paste).
            let editable = params.is_some_and(|p| p.is_editable() == 1);
            if self.kind != ViewKind::Content
                && !editable
                && let Some(model) = model
            {
                model.clear();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::origin_of;

    #[test]
    fn origins() {
        assert_eq!(origin_of("https://a.test/x?y"), "https://a.test");
        assert_eq!(origin_of("https://a.test:8080"), "https://a.test:8080");
        assert_eq!(origin_of("bmcp://ui/newtab.html"), "bmcp://ui");
        assert_eq!(origin_of("about:blank"), "about:blank");
        assert_eq!(origin_of(""), "");
    }
}
