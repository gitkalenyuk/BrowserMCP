//! DevTools ("Inspect" in the page context menu) in a window of their own.
//!
//! The DevTools browser gets its own client ([`ViewKind::DevTools`]): it must never be
//! registered as a tab, never get our shortcuts or navigation rules, and it is not counted
//! as a window that keeps the app alive — when the last real window closes, DevTools
//! windows are closed too ([`close_all`]).

use cef::*;

use crate::bridge::with_ui;
use crate::chrome::ViewKind;
use crate::profiles::ProfileKey;

/// Open DevTools for `browser`, inspecting the element at `point` (view coordinates).
pub(crate) fn show(browser: &Browser, point: (i32, i32)) {
    let Some(ctx) = with_ui(|ui| ui.shell.clone()) else {
        return;
    };
    let Some(host) = browser.host() else { return };
    let mut client = crate::shell::ShellClient::new(
        ctx.browsers.clone(),
        ViewKind::DevTools,
        ProfileKey::default(),
        false,
        0,
    );
    let settings = BrowserSettings::default();
    let at = Point {
        x: point.0,
        y: point.1,
    };
    // No WindowInfo: a Views-hosted page gets Views-hosted DevTools, which arrive through
    // the page's BrowserViewDelegate (`on_popup_browser_view_created`, is_devtools = 1).
    host.show_dev_tools(None, Some(&mut client), Some(&settings), Some(&at));
}

/// Close every DevTools window (the last browser window closed, or `quit`). UI thread.
pub(crate) fn close_all() {
    let browsers: Vec<Browser> = with_ui(|ui| ui.devtools.values().cloned().collect());
    for mut browser in browsers {
        let window = browser_view_get_for_browser(Some(&mut browser)).and_then(|v| v.window());
        match window {
            Some(window) => window.close(),
            None => {
                if let Some(host) = browser.host() {
                    host.close_browser(1);
                }
            }
        }
    }
}
