//! Browser shell: process handler, per-browser client and handlers, windows.
//!
//! Two window layouts (see [`crate::WindowLayout`]):
//! * `Chrome` (default): the Svelte UI around the tabs' pages + Ctrl+K overlay, see
//!   [`crate::chrome`].
//! * `Single` (Phase 0): one BrowserView per top-level window, title follows the page.
//!
//! Plus off-screen (windowless) tabs for automation, see [`crate::osr`].
//!
//! The app quits when the last browser (of any kind) closes; when the last *window* closes,
//! the remaining off-screen tabs are closed too.

use std::cell::RefCell;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use bmcp_core::{BackendError, BrowserBackend, OpenTabOptions, TabId, WindowTarget};
use cef::*;
use tokio::sync::oneshot;

use crate::bridge::{self, LoadState, PendingTab, ShellContext};
use crate::chrome::{self, ViewKind, protocol};
use crate::profiles::ProfileKey;
use crate::{Style, WindowLayout};

/// Called once on the UI thread when CEF is ready and the first window exists.
pub type ReadyCallback = Box<dyn FnOnce(Arc<dyn BrowserBackend>) + Send>;

pub struct StartConfig {
    pub url: String,
    /// Open the saved session (`data/runtime/session.json`) instead of `url`.
    pub restore: bool,
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
    /// Browsers inside a window (everything except off-screen tabs).
    windowed: AtomicUsize,
}

impl Browsers {
    /// Live browsers of any kind.
    pub(crate) fn open_count(&self) -> usize {
        self.open.load(Ordering::SeqCst)
    }
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
            // Website languages and spellcheck on the launch profile's context before its
            // first page loads.
            crate::languages::apply_to_global();
            let ctx = ShellContext {
                style: runtime_style(self.config.style),
                layout: self.config.layout,
                browsers: Arc::new(Browsers::default()),
            };
            bridge::with_ui(|ui| ui.shell = Some(ctx.clone()));
            let first = PendingTab::new(&self.config.url, ProfileKey::default(), None);
            if crate::osr::headless() {
                // No window, no chrome UI, no session restore (automation starts clean).
                crate::osr::open_first_tab(first, &ctx);
            } else {
                // The saved session if there is one (Chrome layout only), else the start page.
                let restored = match crate::session::load() {
                    Some(file) if self.config.restore && ctx.layout == WindowLayout::Chrome => {
                        crate::session::restore(file, &ctx)
                    }
                    Some(file) => {
                        // Not restored: keep at least the closed-tabs stack.
                        bridge::with_ui(|ui| ui.session.closed = file.closed);
                        false
                    }
                    None => false,
                };
                if !restored {
                    open_window(first, &ctx);
                }
            }

            let ready = self.config.on_ready.lock().unwrap_or_else(|e| e.into_inner()).take();
            if let Some(ready) = ready {
                ready(Arc::new(crate::backend::CefBackend));
            }
        }
    }
}

/// Open a new top-level window whose first tab is `pending`, in the configured layout.
/// `pending.reply` learns the id of that tab.
pub(crate) fn open_window(pending: PendingTab, ctx: &ShellContext) {
    match ctx.layout {
        WindowLayout::Chrome => chrome::open_window(pending, ctx),
        WindowLayout::Single => {
            let url = pending.initial_url().to_owned();
            if let Some(view) = create_browser_view(&url, ViewKind::Content, ctx, &pending.profile)
            {
                let view_id = bridge::next_view_id();
                view.set_id(view_id);
                let pending = PendingTab {
                    window: None,
                    index: None,
                    activate: true,
                    ..pending
                };
                bridge::with_ui(|ui| ui.pending.insert(view_id, pending));
                open_single_window(view, ctx.style);
            }
        }
    }
}

/// `tabs.open`: a tab in the current window, a new window, or off-screen.
pub(crate) fn open_tab(
    url: String,
    options: OpenTabOptions,
    reply: oneshot::Sender<TabId>,
) -> Result<(), BackendError> {
    let ctx = bridge::with_ui(|ui| ui.shell.clone()).ok_or(BackendError::ShuttingDown)?;
    let profile = ProfileKey::from_options(options.profile.as_deref(), options.incognito)?;
    let mut pending = PendingTab::new(&url, profile.clone(), options.run);
    pending.activate = !options.background;
    pending.reply = Some(reply);
    let window = options.window;
    // A fresh profile's request context must finish initializing before a browser can use it.
    crate::profiles::when_ready(
        &profile,
        Box::new(move || {
            let result = match (window, ctx.layout) {
                (WindowTarget::Offscreen, _) => crate::osr::open(pending, &ctx),
                (WindowTarget::New, _) | (WindowTarget::Current, WindowLayout::Single) => {
                    open_window(pending, &ctx);
                    Ok(())
                }
                (WindowTarget::Current, WindowLayout::Chrome) => chrome::open_tab(pending),
            };
            if let Err(err) = result {
                // `tabs.open` then fails: its reply channel was dropped with `pending`.
                tracing::error!(%err, "tab not created");
            }
        }),
    );
    Ok(())
}

/// A popup we do not let CEF create (its opener is off-screen, or its profile wants new tabs
/// prepared first): open `url` as our own tab next to the opener instead. The popup loses
/// `window.opener` (documented trade-off). UI thread, outside CEF callbacks.
fn reopen_popup(opener: TabId, url: String, background: bool) {
    let Some(ctx) = bridge::with_ui(|ui| ui.shell.clone()) else {
        return;
    };
    let entry = bridge::with_ui(|ui| {
        let t = ui.tabs.get(&opener)?;
        Some((t.profile.clone(), t.run.clone(), t.window, t.offscreen))
    });
    let Some((profile, run, window, offscreen)) = entry else {
        return;
    };
    let mut pending = PendingTab::new(&url, profile, run);
    pending.activate = !background;
    // The opener's context is initialized already (it has a tab).
    if offscreen {
        if let Err(err) = crate::osr::open(pending, &ctx) {
            tracing::error!(%err, "popup not re-opened");
        }
    } else if let Some(key) = window {
        chrome::add_tab_after(&ctx, key, opener, pending);
    } else {
        open_window(pending, &ctx);
    }
}

/// Create a BrowserView of the given kind in `profile`'s request context. The browser itself
/// is created by CEF once the view is added to a window.
pub(crate) fn create_browser_view(
    url: &str,
    kind: ViewKind,
    ctx: &ShellContext,
    profile: &ProfileKey,
) -> Option<BrowserView> {
    let mut client = ShellClient::new(ctx.browsers.clone(), kind, profile.clone(), false, 0);
    let mut view_delegate = ShellBrowserViewDelegate::new(ctx.clone(), kind);
    let mut settings = BrowserSettings::default();
    if kind != ViewKind::Content {
        // Dark background so our chrome does not flash white before its page paints.
        settings.background_color = 0xFF20_2124;
    }
    // Our own views always use the global context (where bmcp://ui is served from).
    let mut context = match kind {
        ViewKind::Content => crate::profiles::context_for(profile),
        _ => None,
    };
    browser_view_create(
        Some(&mut client),
        Some(&CefString::from(url)),
        Some(&settings),
        None,
        context.as_mut(),
        Some(&mut view_delegate),
    )
}

/// Close every off-screen tab (the last window closed). UI thread.
fn close_offscreen_tabs() {
    let browsers: Vec<Browser> = bridge::with_ui(|ui| {
        ui.tabs
            .values()
            .filter(|t| t.offscreen)
            .map(|t| t.browser.clone())
            .collect()
    });
    for browser in browsers {
        if let Some(host) = browser.host() {
            host.close_browser(1);
        }
    }
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
                ViewKind::DevTools => Size { width: 1100, height: 760 },
            }
        }
    }

    impl BrowserViewDelegate {
        fn delegate_for_popup_browser_view(
            &self,
            _browser_view: Option<&mut BrowserView>,
            _settings: Option<&BrowserSettings>,
            _client: Option<&mut Client>,
            is_devtools: i32,
        ) -> Option<BrowserViewDelegate> {
            // Popups are web pages, whatever opened them (or DevTools, "Inspect").
            let kind = if is_devtools == 1 { ViewKind::DevTools } else { ViewKind::Content };
            Some(ShellBrowserViewDelegate::new(self.ctx.clone(), kind))
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
    // Handlers of one browser. `profile` = its request context's profile (popups share
    // their opener's client); `offscreen` = windowless (has a render handler); `pending` =
    // key of its PendingTab for off-screen tabs (0 otherwise; views are found by view id).
    pub(crate) struct ShellClient {
        browsers: Arc<Browsers>,
        kind: ViewKind,
        profile: ProfileKey,
        offscreen: bool,
        pending: i32,
    }

    impl Client {
        fn display_handler(&self) -> Option<DisplayHandler> {
            Some(ShellDisplayHandler::new(self.kind))
        }

        fn life_span_handler(&self) -> Option<LifeSpanHandler> {
            Some(ShellLifeSpanHandler::new(
                self.browsers.clone(),
                self.kind,
                self.offscreen,
                self.pending,
            ))
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
            Some(ShellContextMenuHandler::new(self.kind, self.offscreen))
        }

        fn permission_handler(&self) -> Option<PermissionHandler> {
            Some(ShellPermissionHandler::new(self.kind))
        }

        fn focus_handler(&self) -> Option<FocusHandler> {
            (self.kind == ViewKind::Content).then(ShellFocusHandler::new)
        }

        fn download_handler(&self) -> Option<DownloadHandler> {
            (self.kind == ViewKind::Content).then(crate::downloads::handler)
        }

        fn render_handler(&self) -> Option<RenderHandler> {
            self.offscreen.then(crate::osr::render_handler)
        }

        fn jsdialog_handler(&self) -> Option<JsdialogHandler> {
            self.offscreen.then(crate::osr::dialog_handler)
        }
    }
}

wrap_permission_handler! {
    struct ShellPermissionHandler {
        kind: ViewKind,
    }

    impl PermissionHandler {
        /// Alloy shows no permission UI, and an unanswered prompt waits forever — e.g.
        /// every connection from a page to 127.0.0.1 (Local Network Access) hung, so the
        /// UI could never reach the API. Our own pages see `scheme::allow_permission`; web
        /// pages get the stored rules (`permissions.set`), and without a rule the browser
        /// UI's permission bar asks (`prompts.rs`; off-screen tabs: denied).
        fn on_show_permission_prompt(
            &self,
            browser: Option<&mut Browser>,
            prompt_id: u64,
            requesting_origin: Option<&CefString>,
            requested_permissions: u32,
            callback: Option<&mut PermissionPromptCallback>,
        ) -> i32 {
            let Some(callback) = callback else { return 0 };
            let origin = requesting_origin.map(CefString::to_string).unwrap_or_default();
            // Our own pages — the chrome, the palette, and bmcp://ui pages shown in a tab
            // (library, settings) — may reach the API on 127.0.0.1 (Local Network Access).
            let ours = crate::scheme::allow_permission(&origin, requested_permissions);
            let allow = match (self.kind, browser) {
                _ if ours => Some(true),
                (ViewKind::Content, Some(browser)) => {
                    let kinds =
                        bmcp_profiles::permissions::kinds_from_prompt_bits(requested_permissions);
                    let decided = crate::prompts::decide(browser, &origin, &kinds);
                    if decided.is_none() {
                        // Nobody decided yet: the permission bar asks the person.
                        let answer = crate::prompts::Answer::Prompt(callback.clone());
                        crate::prompts::wait(browser, Some(prompt_id), &origin, kinds, answer);
                    }
                    decided
                }
                _ => Some(false),
            };
            tracing::debug!(origin, requested_permissions, ?allow, "permission prompt");
            if let Some(allow) = allow {
                callback.cont(if allow {
                    PermissionRequestResult::ACCEPT
                } else {
                    PermissionRequestResult::DENY
                });
            }
            1
        }

        /// CEF dropped a prompt itself (the page navigated or went away).
        fn on_dismiss_permission_prompt(
            &self,
            _browser: Option<&mut Browser>,
            prompt_id: u64,
            _result: PermissionRequestResult,
        ) {
            crate::prompts::dismissed(prompt_id);
        }

        /// Camera / microphone / screen capture (`getUserMedia`, `getDisplayMedia`): the
        /// rules decide, else the permission bar asks (Alloy would grant it otherwise, and
        /// pages could then read device labels); off-screen tabs are denied.
        fn on_request_media_access_permission(
            &self,
            browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            requesting_origin: Option<&CefString>,
            requested_permissions: u32,
            callback: Option<&mut MediaAccessCallback>,
        ) -> i32 {
            let Some(callback) = callback else { return 0 };
            let origin = requesting_origin.map(CefString::to_string).unwrap_or_default();
            let kinds = bmcp_profiles::permissions::kinds_from_media_bits(requested_permissions);
            let allow = match (self.kind, browser) {
                (ViewKind::Content, Some(browser)) => {
                    let decided = crate::prompts::decide(browser, &origin, &kinds);
                    if decided.is_none() {
                        let answer =
                            crate::prompts::Answer::Media(callback.clone(), requested_permissions);
                        crate::prompts::wait(browser, None, &origin, kinds, answer);
                    }
                    decided
                }
                _ => Some(false),
            };
            tracing::debug!(origin, requested_permissions, ?allow, "media permission");
            // All or nothing: a partial grant is invalid for getUserMedia.
            if let Some(allow) = allow {
                callback.cont(if allow { requested_permissions } else { 0 });
            }
            1
        }
    }
}

/// A request's transition type as raw bits (`cef_transition_type_t`): one source value OR-ed
/// with qualifier flags. Such a combination is not one of the Rust enum's variants, so it is
/// read through the C function as a plain integer instead of `Request::transition_type()`.
fn transition_bits(request: &Request) -> u32 {
    let raw = ImplRequest::get_raw(request);
    if raw.is_null() {
        return 0;
    }
    // SAFETY: `raw` is a live cef_request_t (we hold a reference); the C function returns a
    // 32-bit enum, which has the same ABI as an i32.
    unsafe {
        let Some(get) = (*raw).get_transition_type else {
            return 0;
        };
        let get: unsafe extern "C" fn(*mut sys::_cef_request_t) -> i32 = std::mem::transmute(get);
        get(raw) as u32
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

wrap_focus_handler! {
    struct ShellFocusHandler;

    impl FocusHandler {
        /// A page got keyboard focus: in a split view that half becomes the active tab.
        fn on_got_focus(&self, browser: Option<&mut Browser>) {
            if let Some(browser) = browser {
                let id = TabId(browser.identifier());
                bridge::post_to_ui(Box::new(move || chrome::pane_focused(id)));
            }
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
                bridge::update_tab(browser, |tab| {
                    // A new site: its favicon is unknown until it reports one.
                    if origin_of(&tab.url) != origin_of(&url) {
                        tab.favicon = None;
                    }
                    tab.url = url;
                    true
                });
                // The host's remembered zoom (outside this callback).
                let id = TabId(browser.identifier());
                bridge::post_to_ui(Box::new(move || crate::zoom::apply_stored(id)));
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
            if self.kind == ViewKind::Ui || self.kind == ViewKind::Palette {
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
        offscreen: bool,
        pending: i32,
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
                let Some(opener) = browser else { return 0 };
                let background = target_disposition == WindowOpenDisposition::NEW_BACKGROUND_TAB;
                let opener_id = TabId(opener.identifier());
                // Off-screen openers have no window to put a CEF popup in, and tabs of a
                // profile with an identity must get it before their first script: open the
                // URL as our own tab instead.
                let profile = bridge::with_ui(|ui| ui.tabs.get(&opener_id).map(|t| t.profile.name()));
                let reopen = self.offscreen
                    || profile.is_some_and(|p| crate::hooks::wants_prepare(&p));
                if reopen && (url.starts_with("http") || url.is_empty()) {
                    let url = if url.is_empty() { "about:blank".to_owned() } else { url };
                    bridge::post_to_ui(Box::new(move || reopen_popup(opener_id, url, background)));
                    return 1;
                }
                // Allowed; it becomes a tab in on_popup_browser_view_created. Remember
                // whether it should open in the background (Ctrl+click, middle click).
                bridge::with_ui(|ui| ui.popups.insert(opener.identifier(), (background, url)));
                return 0;
            }
            // Our chrome pages never open windows.
            tracing::warn!(url, "popup from a chrome view blocked");
            1
        }

        fn on_after_created(&self, browser: Option<&mut Browser>) {
            if let Some(browser) = browser {
                match self.kind {
                    ViewKind::Content => bridge::register_tab(browser, self.offscreen, self.pending),
                    ViewKind::DevTools => bridge::with_ui(|ui| {
                        ui.devtools.insert(browser.identifier(), browser.clone());
                    }),
                    kind => bridge::register_chrome_view(browser, kind),
                }
            }
            let open = self.browsers.open.fetch_add(1, Ordering::SeqCst) + 1;
            if self.keeps_app_alive() {
                self.browsers.windowed.fetch_add(1, Ordering::SeqCst);
            }
            tracing::debug!(open, kind = ?self.kind, offscreen = self.offscreen, "browser created");
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
                    ViewKind::DevTools => {
                        let removed = bridge::with_ui(|ui| ui.devtools.remove(&browser.identifier()));
                        drop(removed);
                    }
                    _ => bridge::unregister_chrome_view(browser),
                }
            }
            let open = self.browsers.open.fetch_sub(1, Ordering::SeqCst) - 1;
            tracing::debug!(open, kind = ?self.kind, "browser closed");
            if open == 0 {
                // Headless: the automation host outlives its last tab (until system.quit).
                if !crate::osr::keep_alive_without_browsers() {
                    quit_message_loop();
                    crate::cdp_port::release_keep_alive(); // else --cdp-port keeps us alive
                }
            } else if self.keeps_app_alive()
                && self.browsers.windowed.fetch_sub(1, Ordering::SeqCst) == 1
            {
                // The last window is gone: off-screen tabs and DevTools windows must not
                // keep the app alive.
                bridge::post_to_ui(Box::new(|| {
                    close_offscreen_tabs();
                    crate::devtools::close_all();
                }));
            }
        }
    }
}

impl ShellLifeSpanHandler {
    /// Browsers in a real window count as "a window is open"; off-screen tabs and DevTools
    /// do not.
    fn keeps_app_alive(&self) -> bool {
        !self.offscreen && self.kind != ViewKind::DevTools
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
            if self.kind == ViewKind::Ui && is_loading == 1 {
                // A reloading chrome page has no dropdown open any more.
                chrome::chrome_reloaded(browser);
            }
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
            if self.kind == ViewKind::DevTools {
                return 0;
            }
            // Releasing Ctrl ends a Ctrl+wheel zoom: read the zoom back (zoom.rs).
            if event.type_ == KeyEventType::KEYUP
                && event.windows_key_code == protocol::VK_CONTROL
                && let Some(key) = chrome::window_key(browser)
            {
                bridge::post_to_ui(Box::new(move || crate::zoom::check_after_ctrl(key)));
            }
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
            is_redirect: i32,
        ) -> i32 {
            let (Some(browser), Some(frame), Some(request)) = (browser, frame, request) else {
                return 0;
            };
            let url = CefString::from(&request.url()).to_string();
            if self.kind == ViewKind::DevTools {
                return 0; // devtools:// pages
            }
            if self.kind == ViewKind::Content {
                // A tab may show our exact new tab page — when the browser itself loads it —
                // but never the chrome/palette pages (they have full API access), never
                // bmcp://cmd, and a web page can never send a tab to bmcp:// at all.
                let current = CefString::from(&frame.url()).to_string();
                let transition = transition_bits(request);
                let allowed = crate::scheme::content_navigation_allowed(
                    &url,
                    &current,
                    transition,
                    is_redirect == 1,
                    frame.is_main() == 1,
                );
                if !allowed {
                    tracing::warn!(url, current, transition, "blocked bmcp:// navigation in a tab");
                    return 1;
                }
                if frame.is_main() == 1 {
                    // Site policy js/images → content settings, before the page commits.
                    crate::net::apply_content_settings(browser, &url);
                }
                return 0;
            }
            if frame.is_main() == 0 || crate::scheme::is_chrome_url(&url) {
                return 0;
            }
            // Our chrome pages stay put. A link dropped on the UI (or any other attempt
            // to leave bmcp://ui/) opens in the shown tab instead.
            chrome::open_in_content(browser, &url);
            1
        }

        /// Every request of a web page goes through the adblock / site-policy gate
        /// (`net.rs`). Our own views are never filtered.
        fn resource_request_handler(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _request: Option<&mut Request>,
            _is_navigation: ::std::os::raw::c_int,
            _is_download: ::std::os::raw::c_int,
            _request_initiator: Option<&CefString>,
            _disable_default_handling: Option<&mut ::std::os::raw::c_int>,
        ) -> Option<ResourceRequestHandler> {
            if self.kind != ViewKind::Content {
                return None;
            }
            crate::net::request_handler()
        }
    }
}

wrap_context_menu_handler! {
    struct ShellContextMenuHandler {
        kind: ViewKind,
        offscreen: bool,
    }

    impl ContextMenuHandler {
        fn on_before_context_menu(
            &self,
            browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            params: Option<&mut ContextMenuParams>,
            model: Option<&mut MenuModel>,
        ) {
            let (Some(params), Some(model)) = (params, model) else { return };
            match self.kind {
                // Web pages: our own menu (menu.rs), drawn natively over the page.
                ViewKind::Content => {
                    if let Some(browser) = browser {
                        crate::menu::fill(browser, params, model);
                    }
                }
                ViewKind::DevTools => {}
                // Alloy's default menu (Back, Reload, View source, ...) makes no sense on
                // our chrome; keep it only for text fields (cut/copy/paste).
                ViewKind::Ui | ViewKind::Palette => {
                    if params.is_editable() != 1 {
                        model.clear();
                    }
                }
            }
        }

        /// Windowed tabs: CEF draws the native menu. Off-screen tabs have no screen for it
        /// (see `menu::run_offscreen`).
        fn run_context_menu(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _params: Option<&mut ContextMenuParams>,
            model: Option<&mut MenuModel>,
            callback: Option<&mut RunContextMenuCallback>,
        ) -> ::std::os::raw::c_int {
            match (self.offscreen, model, callback) {
                (true, Some(model), Some(callback)) => {
                    crate::menu::run_offscreen(model, callback);
                    1
                }
                _ => 0,
            }
        }

        fn on_context_menu_command(
            &self,
            browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _params: Option<&mut ContextMenuParams>,
            command_id: ::std::os::raw::c_int,
            _event_flags: EventFlags,
        ) -> ::std::os::raw::c_int {
            match (self.kind, browser) {
                (ViewKind::Content, Some(browser)) => crate::menu::run(browser, command_id),
                _ => 0,
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
