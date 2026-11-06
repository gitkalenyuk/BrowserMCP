//! Browser shell: process handler, per-browser client and handlers, windows.
//!
//! Two window layouts (see [`crate::WindowLayout`]):
//! * `Chrome` (spike S1, default): UI strip + page + Ctrl+K overlay, see [`crate::chrome`].
//! * `Single` (Phase 0): one BrowserView per top-level window, title follows the page.
//!
//! The app quits when the last browser (of any kind) closes.

use std::cell::RefCell;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use bmcp_core::BrowserBackend;
use cef::*;

use crate::bridge::{self, LoadState, ShellContext};
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
            open_window(&self.config.url, &ctx);

            let ready = self.config.on_ready.lock().unwrap_or_else(|e| e.into_inner()).take();
            if let Some(ready) = ready {
                ready(Arc::new(crate::backend::CefBackend));
            }
        }
    }
}

/// Open a new top-level window showing `url`, in the configured layout.
pub(crate) fn open_window(url: &str, ctx: &ShellContext) {
    match ctx.layout {
        WindowLayout::Chrome => chrome::open_window(url, ctx),
        WindowLayout::Single => {
            if let Some(view) = create_browser_view(url, ViewKind::Content, ctx) {
                open_single_window(view, ctx.style);
            }
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
            // back to the BrowserView default (which made the UI strip fill the window).
            // Only the heights matter here: the box layout stretches the width.
            match self.kind {
                ViewKind::Ui => Size { width: 1, height: chrome::UI_HEIGHT },
                ViewKind::Palette => {
                    let (width, height) = protocol::PALETTE_SIZE;
                    Size { width, height }
                }
                // Flex 1 gives the page all remaining space.
                ViewKind::Content => Size { width: 1, height: 1 },
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
            _browser_view: Option<&mut BrowserView>,
            popup_browser_view: Option<&mut BrowserView>,
            is_devtools: i32,
        ) -> i32 {
            let Some(popup) = popup_browser_view.cloned() else { return 0 };
            // Spike: a popup gets a window of its own. Phase 2 turns popups into tabs.
            if self.ctx.layout == WindowLayout::Chrome && is_devtools == 0 {
                chrome::open_window_with(popup, &self.ctx);
            } else {
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
            Some(ShellKeyboardHandler::new())
        }

        fn request_handler(&self) -> Option<RequestHandler> {
            Some(ShellRequestHandler::new(self.kind))
        }

        fn context_menu_handler(&self) -> Option<ContextMenuHandler> {
            Some(ShellContextMenuHandler::new(self.kind))
        }
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
                bridge::update_tab(browser, |tab| tab.url = url);
                chrome::push_state(browser);
            }
        }

        fn on_title_change(&self, browser: Option<&mut Browser>, title: Option<&CefString>) {
            if self.kind != ViewKind::Content {
                return; // our own pages must not rename the window
            }
            let title = title.map(CefString::to_string).unwrap_or_default();
            if let Some(browser) = browser.as_deref() {
                let title = title.clone();
                bridge::update_tab(browser, |tab| tab.title = title);
                chrome::push_state(browser);
            }
            let mut browser = browser.cloned();
            let Some(window) =
                browser_view_get_for_browser(browser.as_mut()).and_then(|view| view.window())
            else {
                return;
            };
            let title = if title.is_empty() {
                "BrowserMCP".to_owned()
            } else {
                format!("{title} — BrowserMCP")
            };
            window.set_title(Some(&CefString::from(title.as_str())));
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
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _popup_id: i32,
            target_url: Option<&CefString>,
            _target_frame_name: Option<&CefString>,
            _target_disposition: WindowOpenDisposition,
            _user_gesture: i32,
            _popup_features: Option<&PopupFeatures>,
            _window_info: Option<&mut WindowInfo>,
            _client: Option<&mut Option<Client>>,
            _settings: Option<&mut BrowserSettings>,
            _extra_info: Option<&mut Option<DictionaryValue>>,
            _no_javascript_access: Option<&mut i32>,
        ) -> i32 {
            if self.kind == ViewKind::Content {
                return 0; // allowed; see on_popup_browser_view_created
            }
            // Our chrome pages never open windows.
            let url = target_url.map(CefString::to_string).unwrap_or_default();
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
            _can_go_back: ::std::os::raw::c_int,
            _can_go_forward: ::std::os::raw::c_int,
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
            });
            chrome::push_state(browser);
        }
    }
}

wrap_keyboard_handler! {
    struct ShellKeyboardHandler;

    impl KeyboardHandler {
        /// Runs before the page (or our UI) sees the key, whichever view has focus.
        fn on_pre_key_event(
            &self,
            browser: Option<&mut Browser>,
            event: Option<&KeyEvent>,
            _os_event: Option<&mut sys::MSG>,
            _is_keyboard_shortcut: Option<&mut i32>,
        ) -> i32 {
            let (Some(browser), Some(event)) = (browser, event) else { return 0 };
            tracing::trace!(
                kind = ?event.type_,
                code = event.windows_key_code,
                modifiers = event.modifiers,
                "pre key event"
            );
            if event.type_ != KeyEventType::RAWKEYDOWN {
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
            if self.kind == ViewKind::Content {
                return 0;
            }
            let (Some(browser), Some(frame), Some(request)) = (browser, frame, request) else {
                return 0;
            };
            let url = CefString::from(&request.url()).to_string();
            if frame.is_main() == 0 || url.starts_with("bmcp://ui/") {
                return 0;
            }
            // Our chrome pages stay put. A link dropped on the UI strip (or any other
            // attempt to leave bmcp://ui/) opens in the page instead.
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
