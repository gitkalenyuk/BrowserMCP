//! Off-screen tabs (`tabs.open {window: "offscreen"}`): windowless browsers (CEF off-screen
//! rendering) for automation, and headless mode (`browsermcp --headless`).
//!
//! Why: a background tab of a window is *hidden* — Chromium ignores input to it and the
//! page reports `visibilityState: "hidden"` — and only one page of a window can have the
//! keyboard focus. Parallel mass-run rows each typing into their own page therefore need
//! pages that are visible and focused at the same time, without fighting over the desktop:
//! a windowless browser has its own size (from [`OffscreenRender::view_rect`]), its own
//! focus (`set_focus`) and is never hidden, and nothing appears on screen. Input goes
//! through CDP as usual; screenshots (`Page.captureScreenshot`) work too.
//!
//! Such tabs have no window (`TabEntry::window = None`, `offscreen = true`). They are closed
//! with the browser host, and all of them when the last real window closes.
//!
//! Headless ([`configure_headless`]): the browser never creates a window or a view — no
//! chrome UI, no palette. The first tab and every `tabs.open` (whatever `window` says) are
//! off-screen tabs, popups of off-screen pages are re-opened as off-screen tabs, and the
//! process is an automation host: closing the last tab keeps it running until `system.quit`
//! ([`begin_quit`]).

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use bmcp_core::BackendError;
use cef::*;

use crate::bridge::{PendingTab, ShellContext, with_ui};
use crate::chrome::ViewKind;

/// Default size of an off-screen page (a common laptop viewport).
pub(crate) const VIEW_WIDTH: i32 = 1280;
pub(crate) const VIEW_HEIGHT: i32 = 800;

/// Page size and device pixel ratio of every off-screen tab.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HeadlessOptions {
    pub width: u32,
    pub height: u32,
    /// `window.devicePixelRatio`; screenshots are this many device pixels per CSS pixel.
    pub device_scale: f32,
}

impl Default for HeadlessOptions {
    fn default() -> Self {
        Self {
            width: VIEW_WIDTH as u32,
            height: VIEW_HEIGHT as u32,
            device_scale: 1.0,
        }
    }
}

/// Set once at startup by [`configure_headless`] (`None` = windowed browser).
static HEADLESS: OnceLock<HeadlessOptions> = OnceLock::new();
/// `system.quit` was called: the last browser closing ends the message loop again.
static QUITTING: AtomicBool = AtomicBool::new(false);

/// Headless mode on, with this page size and scale. Before CEF initializes.
pub(crate) fn configure_headless(options: HeadlessOptions) {
    let _ = HEADLESS.set(options);
}

/// Is this a headless browser (no windows at all)?
pub(crate) fn headless() -> bool {
    HEADLESS.get().is_some()
}

/// Should the message loop keep running although no browser is left? Only in headless mode
/// (an automation host without tabs), until [`begin_quit`].
pub(crate) fn keep_alive_without_browsers() -> bool {
    headless() && !QUITTING.load(Ordering::SeqCst)
}

/// `quit` in headless mode: from now on the last browser closing ends the loop; with no
/// browser left at all, end it right away. UI thread.
pub(crate) fn begin_quit(open_browsers: usize) {
    QUITTING.store(true, Ordering::SeqCst);
    if open_browsers == 0 {
        tracing::info!("headless: no tab left, ending the message loop");
        quit_message_loop();
        crate::cdp_port::release_keep_alive();
    }
}

fn view_size() -> (i32, i32) {
    let o = HEADLESS.get().copied().unwrap_or_default();
    (o.width as i32, o.height as i32)
}

fn device_scale() -> f32 {
    HEADLESS.get().map_or(1.0, |o| o.device_scale)
}

/// Create a windowless browser for `pending`. It registers like any tab: its client carries
/// the key of `pending` in `UiState::pending` (there is no view id to find it by). Created
/// asynchronously, because a new profile's request context may still be initializing (the
/// synchronous variant fails then). UI thread.
pub(crate) fn open(pending: PendingTab, ctx: &ShellContext) -> Result<(), BackendError> {
    let url = pending.initial_url().to_owned();
    let profile = pending.profile.clone();
    let mut context = crate::profiles::context_for(&profile);
    let key = crate::bridge::next_view_id();
    let mut client =
        crate::shell::ShellClient::new(ctx.browsers.clone(), ViewKind::Content, profile, true, key);
    let (width, height) = view_size();
    let window_info = WindowInfo {
        windowless_rendering_enabled: 1,
        // Off-screen rendering needs the Alloy runtime style.
        runtime_style: RuntimeStyle::ALLOY,
        bounds: Rect {
            x: 0,
            y: 0,
            width,
            height,
        },
        ..Default::default()
    };
    let settings = BrowserSettings {
        // Frames are thrown away; a low rate still drives rAF, timers, input and the frames
        // of `Page.startScreencast` (record.video_*).
        windowless_frame_rate: 30,
        ..Default::default()
    };
    // Stored before creating: `on_after_created` may even run inside the call below.
    with_ui(|ui| ui.pending.insert(key, pending));
    let started = browser_host_create_browser(
        Some(&window_info),
        Some(&mut client),
        Some(&CefString::from(url.as_str())),
        Some(&settings),
        None,
        context.as_mut(),
    );
    if started != 1 {
        let dropped = with_ui(|ui| ui.pending.remove(&key));
        drop(dropped);
        return Err(BackendError::Cdp(
            "could not create an off-screen browser".into(),
        ));
    }
    Ok(())
}

/// Headless startup: the first tab instead of the first window. UI thread.
pub(crate) fn open_first_tab(pending: PendingTab, ctx: &ShellContext) {
    let (width, height) = view_size();
    tracing::info!(
        width,
        height,
        scale = device_scale(),
        "headless: no windows"
    );
    if let Err(err) = open(pending, ctx) {
        tracing::error!(%err, "headless: first tab not created");
    }
}

/// The render handler of every off-screen browser.
pub(crate) fn render_handler() -> RenderHandler {
    OffscreenRender::new()
}

/// The JavaScript dialog handler of every off-screen browser.
///
/// Pitfall: a windowless browser has no dialog runner, so CEF silently *cancelled* every
/// `alert`/`confirm`/`prompt`/`beforeunload` (confirm() returned false at once) — although
/// DevTools had already reported `Page.javascriptDialogOpening`, so `page.dialog` then found
/// nothing to answer. Claiming the dialog here (and holding its callback) keeps it open like
/// in a window. `Page.handleJavaScriptDialog` (`page.dialog`, the dialog policies) is then
/// answered through the held callback ([`answer_dialog`], called by the backend), the same
/// path as a click in a window's dialog.
pub(crate) fn dialog_handler() -> JsdialogHandler {
    OffscreenDialogs::new()
}

thread_local! {
    /// Open dialogs of off-screen browsers by browser id (UI thread). Dropping a callback
    /// without running it would cancel the dialog, so it is held until the dialog closes.
    static OPEN_DIALOGS: std::cell::RefCell<std::collections::HashMap<i32, JsdialogCallback>> =
        Default::default();
}

fn hold_dialog(browser: Option<&mut Browser>, callback: Option<&mut JsdialogCallback>) -> i32 {
    let (Some(browser), Some(callback)) = (browser, callback) else {
        return 0;
    };
    let id = browser.identifier();
    OPEN_DIALOGS.with(|d| d.borrow_mut().insert(id, callback.clone()));
    1
}

fn forget_dialog(browser: Option<&mut Browser>) {
    if let Some(browser) = browser {
        let id = browser.identifier();
        let held = OPEN_DIALOGS.with(|d| d.borrow_mut().remove(&id));
        drop(held);
    }
}

/// Answer the open dialog of off-screen tab `browser_id` directly (UI thread). False when it
/// has none (it was answered through CDP already, or it is not an off-screen tab).
pub(crate) fn answer_dialog(browser_id: i32, accept: bool, text: Option<&str>) -> bool {
    let held = OPEN_DIALOGS.with(|d| d.borrow_mut().remove(&browser_id));
    match held {
        Some(callback) => {
            let text = text.map(CefString::from);
            callback.cont(i32::from(accept), text.as_ref());
            true
        }
        None => false,
    }
}

wrap_jsdialog_handler! {
    struct OffscreenDialogs;

    impl JsdialogHandler {
        fn on_jsdialog(
            &self,
            browser: Option<&mut Browser>,
            _origin_url: Option<&CefString>,
            _dialog_type: JsdialogType,
            _message_text: Option<&CefString>,
            _default_prompt_text: Option<&CefString>,
            callback: Option<&mut JsdialogCallback>,
            suppress_message: Option<&mut ::std::os::raw::c_int>,
        ) -> ::std::os::raw::c_int {
            if let Some(suppress) = suppress_message {
                *suppress = 0;
            }
            hold_dialog(browser, callback)
        }

        fn on_before_unload_dialog(
            &self,
            browser: Option<&mut Browser>,
            _message_text: Option<&CefString>,
            _is_reload: ::std::os::raw::c_int,
            callback: Option<&mut JsdialogCallback>,
        ) -> ::std::os::raw::c_int {
            hold_dialog(browser, callback)
        }

        fn on_reset_dialog_state(&self, browser: Option<&mut Browser>) {
            forget_dialog(browser);
        }

        fn on_dialog_closed(&self, browser: Option<&mut Browser>) {
            forget_dialog(browser);
        }
    }
}

wrap_render_handler! {
    struct OffscreenRender;

    impl RenderHandler {
        fn view_rect(&self, _browser: Option<&mut Browser>, rect: Option<&mut Rect>) {
            if let Some(rect) = rect {
                let (width, height) = view_size();
                *rect = Rect { x: 0, y: 0, width, height };
            }
        }

        /// The device pixel ratio (`--device-scale`); the "screen" is the page's size.
        fn screen_info(
            &self,
            _browser: Option<&mut Browser>,
            screen_info: Option<&mut ScreenInfo>,
        ) -> ::std::os::raw::c_int {
            let Some(info) = screen_info else { return 0 };
            let (width, height) = view_size();
            let rect = Rect { x: 0, y: 0, width, height };
            info.device_scale_factor = device_scale();
            info.depth = 24;
            info.depth_per_component = 8;
            info.is_monochrome = 0;
            info.rect = rect.clone();
            info.available_rect = rect;
            1
        }

        /// Frames are not shown anywhere (screenshots come from CDP).
        fn on_paint(
            &self,
            _browser: Option<&mut Browser>,
            _type_: PaintElementType,
            _dirty_rects: Option<&[Rect]>,
            _buffer: *const u8,
            _width: ::std::os::raw::c_int,
            _height: ::std::os::raw::c_int,
        ) {
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_options_are_the_offscreen_size() {
        let o = HeadlessOptions::default();
        assert_eq!((o.width, o.height), (1280, 800));
        assert_eq!(o.device_scale, 1.0);
    }
}
