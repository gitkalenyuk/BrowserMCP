//! Off-screen tabs (`tabs.open {window: "offscreen"}`): windowless browsers (CEF off-screen
//! rendering) for automation.
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

use bmcp_core::BackendError;
use cef::*;

use crate::bridge::{PendingTab, ShellContext, with_ui};
use crate::chrome::ViewKind;

/// Size of an off-screen page (a common laptop viewport).
pub(crate) const VIEW_WIDTH: i32 = 1280;
pub(crate) const VIEW_HEIGHT: i32 = 800;

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
    let window_info = WindowInfo {
        windowless_rendering_enabled: 1,
        // Off-screen rendering needs the Alloy runtime style.
        runtime_style: RuntimeStyle::ALLOY,
        bounds: Rect {
            x: 0,
            y: 0,
            width: VIEW_WIDTH,
            height: VIEW_HEIGHT,
        },
        ..Default::default()
    };
    let settings = BrowserSettings {
        // Frames are thrown away; a low rate still drives rAF, timers and input.
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

/// The render handler of every off-screen browser.
pub(crate) fn render_handler() -> RenderHandler {
    OffscreenRender::new()
}

wrap_render_handler! {
    struct OffscreenRender;

    impl RenderHandler {
        fn view_rect(&self, _browser: Option<&mut Browser>, rect: Option<&mut Rect>) {
            if let Some(rect) = rect {
                *rect = Rect {
                    x: 0,
                    y: 0,
                    width: VIEW_WIDTH,
                    height: VIEW_HEIGHT,
                };
            }
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
