//! Phase 0 shell: one top-level Views window per browser, title follows the page,
//! the app quits when the last browser closes. Tabs, the Svelte UI view and the
//! command bridge replace this in later phases.

use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use cef::*;

use crate::Style;

pub struct StartConfig {
    pub url: String,
    pub style: Style,
}

fn runtime_style(style: Style) -> RuntimeStyle {
    match style {
        Style::Chrome => RuntimeStyle::CHROME,
        Style::Alloy => RuntimeStyle::ALLOY,
    }
}

/// Number of live browsers; when it drops to zero the message loop quits.
#[derive(Default)]
struct Browsers {
    open: AtomicUsize,
}

wrap_app! {
    pub struct ShellApp {
        config: Arc<StartConfig>,
    }

    impl App {
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
            let style = runtime_style(self.config.style);
            let browsers = Arc::new(Browsers::default());
            open_window(&self.config.url, style, browsers);
        }
    }
}

/// Create a BrowserView for `url` and put it in a new top-level window.
fn open_window(url: &str, style: RuntimeStyle, browsers: Arc<Browsers>) {
    let mut client = ShellClient::new(browsers.clone());
    let mut view_delegate = ShellBrowserViewDelegate::new(style, browsers);
    let browser_view = browser_view_create(
        Some(&mut client),
        Some(&CefString::from(url)),
        Some(&BrowserSettings::default()),
        None,
        None,
        Some(&mut view_delegate),
    );
    let mut window_delegate = ShellWindowDelegate::new(RefCell::new(browser_view), style);
    window_create_top_level(Some(&mut window_delegate));
}

wrap_window_delegate! {
    struct ShellWindowDelegate {
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

        fn window_runtime_style(&self) -> RuntimeStyle {
            self.style
        }
    }
}

wrap_browser_view_delegate! {
    struct ShellBrowserViewDelegate {
        style: RuntimeStyle,
        browsers: Arc<Browsers>,
    }

    impl ViewDelegate {}

    impl BrowserViewDelegate {
        fn on_popup_browser_view_created(
            &self,
            _browser_view: Option<&mut BrowserView>,
            popup_browser_view: Option<&mut BrowserView>,
            _is_devtools: i32,
        ) -> i32 {
            // Phase 0: popups get their own window. Phase 2 turns them into tabs.
            let mut delegate =
                ShellWindowDelegate::new(RefCell::new(popup_browser_view.cloned()), self.style);
            window_create_top_level(Some(&mut delegate));
            1
        }

        fn browser_runtime_style(&self) -> RuntimeStyle {
            self.style
        }
    }
}

wrap_client! {
    struct ShellClient {
        browsers: Arc<Browsers>,
    }

    impl Client {
        fn display_handler(&self) -> Option<DisplayHandler> {
            Some(ShellDisplayHandler::new())
        }

        fn life_span_handler(&self) -> Option<LifeSpanHandler> {
            Some(ShellLifeSpanHandler::new(self.browsers.clone()))
        }
    }
}

wrap_display_handler! {
    struct ShellDisplayHandler;

    impl DisplayHandler {
        fn on_title_change(&self, browser: Option<&mut Browser>, title: Option<&CefString>) {
            let mut browser = browser.cloned();
            let Some(window) =
                browser_view_get_for_browser(browser.as_mut()).and_then(|view| view.window())
            else {
                return;
            };
            let title = title.map(CefString::to_string).unwrap_or_default();
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
    }

    impl LifeSpanHandler {
        fn on_after_created(&self, _browser: Option<&mut Browser>) {
            let open = self.browsers.open.fetch_add(1, Ordering::SeqCst) + 1;
            tracing::debug!(open, "browser created");
        }

        fn on_before_close(&self, _browser: Option<&mut Browser>) {
            let open = self.browsers.open.fetch_sub(1, Ordering::SeqCst) - 1;
            tracing::debug!(open, "browser closed");
            if open == 0 {
                quit_message_loop();
            }
        }
    }
}
