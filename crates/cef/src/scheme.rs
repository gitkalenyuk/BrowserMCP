//! The `bmcp://` scheme.
//!
//! * `bmcp://ui/...`  — our own pages: the Svelte chrome (`index.html`), the Ctrl+K palette
//!   (`palette.html`) and the new tab page (`newtab.html`), see [`crate::ui_files`].
//! * `bmcp://cmd/...` — window-local commands from those pages, see
//!   [`crate::chrome::protocol`].
//!
//! A custom scheme must be registered in EVERY process (browser, renderer, GPU, ...)
//! before CEF starts, with the same options: that is what [`register_custom_schemes`]
//! does from `App::on_register_custom_schemes`. Sub-processes therefore get the small
//! [`SchemeApp`] in `execute_process`. The handler that actually serves the bytes
//! ([`register_handler`]) only lives in the browser process.

use std::sync::{Arc, Mutex, OnceLock};

use cef::*;

use crate::chrome::protocol::{self, CMD_PREFIX, percent_encode};
use crate::ui_files::{self, Source};

pub const SCHEME: &str = "bmcp";
/// Origin of our pages.
pub const UI_ORIGIN: &str = "bmcp://ui";
pub const UI_PREFIX: &str = "bmcp://ui/";
/// Pages under `bmcp://ui/`.
pub const UI_PAGE: &str = "index.html";
pub const PALETTE_PAGE: &str = "palette.html";
pub const NEWTAB_PAGE: &str = "newtab.html";

/// Where our pages come from; set once at startup by [`configure`].
struct Config {
    source: Source,
    /// `{"ws": "..."}` injected into every HTML page as `window.__BMCP__`.
    config_json: String,
    /// Vite dev server base (`http://localhost:5173`) for the chrome and palette.
    dev_url: Option<String>,
    api_ws: Option<String>,
}

static CONFIG: OnceLock<Config> = OnceLock::new();

fn config() -> &'static Config {
    CONFIG.get_or_init(|| Config {
        source: Source::Embedded,
        config_json: "{}".into(),
        dev_url: None,
        api_ws: None,
    })
}

/// Remember the UI options (call before CEF starts).
pub(crate) fn configure(ui: &crate::UiOptions) {
    let config_json = serde_json::json!({ "ws": ui.api_ws }).to_string();
    let _ = CONFIG.set(Config {
        source: ui.dir.clone().map_or(Source::Embedded, Source::Folder),
        config_json,
        dev_url: ui
            .dev_url
            .as_ref()
            .map(|u| u.trim_end_matches('/').to_owned()),
        api_ws: ui.api_ws.clone(),
    });
}

/// URL of one of our chrome pages (`index.html`, `palette.html`), honouring `--ui-dev`.
pub(crate) fn ui_page_url(page: &str) -> String {
    let config = config();
    match &config.dev_url {
        // The Vite page cannot get the injected config, so it gets the API in the query.
        Some(dev) => format!(
            "{dev}/{page}?api=ws&ws={}",
            percent_encode(config.api_ws.as_deref().unwrap_or(""))
        ),
        None => format!("{UI_PREFIX}{page}"),
    }
}

/// May a chrome view (UI / palette) show this URL?
pub(crate) fn is_chrome_url(url: &str) -> bool {
    url.starts_with(UI_PREFIX)
        || config()
            .dev_url
            .as_deref()
            .is_some_and(|dev| url.starts_with(&format!("{dev}/")))
}

/// May a web page (content view) show this URL? Only the new tab page of our own pages:
/// the chrome and palette pages have no business inside a tab.
pub(crate) fn is_allowed_in_content(url: &str) -> bool {
    match url.strip_prefix(UI_PREFIX) {
        Some(rest) => rest.split(['?', '#']).next() == Some(NEWTAB_PAGE),
        None => !url.starts_with(CMD_PREFIX),
    }
}

/// `cef_permission_request_types_t` bits for "talk to localhost / the local network"
/// (Chromium's Local Network Access): LOCAL_NETWORK_ACCESS (1 << 25), LOCAL_NETWORK
/// (1 << 26), LOOPBACK_NETWORK (1 << 27).
const NETWORK_PERMISSIONS: u32 = (1 << 25) | (1 << 26) | (1 << 27);

/// Answer a permission prompt (Alloy has no prompt UI; unanswered prompts hang).
///
/// Our own pages (`bmcp://ui`, or the `--ui-dev` server) may reach the loopback network:
/// that is how they talk to the API on 127.0.0.1. Everything else is denied for now — web
/// pages must not reach the API or other local services, and a real permission UI is a
/// later Phase 2 item. Returns true to accept.
pub(crate) fn allow_permission(requesting_origin: &str, requested: u32) -> bool {
    let ours = requesting_origin.trim_end_matches('/') == UI_ORIGIN
        || config()
            .dev_url
            .as_deref()
            .is_some_and(|dev| requesting_origin.trim_end_matches('/') == dev);
    ours && requested & !NETWORK_PERMISSIONS == 0 && requested != 0
}

/// Origin allowed to fetch `bmcp://cmd/` (our pages, or the Vite dev server).
fn chrome_origin() -> String {
    config()
        .dev_url
        .clone()
        .unwrap_or_else(|| UI_ORIGIN.to_owned())
}

/// Register `bmcp` as standard + secure + CORS + fetch enabled. Call from
/// `App::on_register_custom_schemes` in every process.
pub(crate) fn register_custom_schemes(registrar: Option<&mut SchemeRegistrar>) {
    let Some(registrar) = registrar else { return };
    let options = SchemeOptions::STANDARD.get_raw()
        | SchemeOptions::SECURE.get_raw()
        | SchemeOptions::CORS_ENABLED.get_raw()
        | SchemeOptions::FETCH_ENABLED.get_raw();
    registrar.add_custom_scheme(Some(&CefString::from(SCHEME)), options);
}

// App for Chromium sub-processes: only registers our scheme.
wrap_app! {
    pub(crate) struct SchemeApp;

    impl App {
        fn on_register_custom_schemes(&self, registrar: Option<&mut SchemeRegistrar>) {
            register_custom_schemes(registrar);
        }
    }
}

/// Install the `bmcp://` handler on the global request context. Browser process, after
/// CEF initialized (`on_context_initialized`).
pub(crate) fn register_handler() {
    let mut factory = BmcpSchemeFactory::new();
    let ok =
        register_scheme_handler_factory(Some(&CefString::from(SCHEME)), None, Some(&mut factory));
    if ok != 1 {
        tracing::error!("could not register the bmcp:// scheme handler");
    }
}

wrap_scheme_handler_factory! {
    struct BmcpSchemeFactory;

    impl SchemeHandlerFactory {
        /// Runs on the IO thread for every `bmcp://` request.
        fn create(
            &self,
            browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _scheme_name: Option<&CefString>,
            request: Option<&mut Request>,
        ) -> Option<ResourceHandler> {
            let url = request.map(|r| CefString::from(&r.url()).to_string()).unwrap_or_default();
            if url.starts_with(CMD_PREFIX) {
                return Some(command_response(browser.map(|b| b.identifier()), &url));
            }
            let file = url.strip_prefix(UI_PREFIX).and_then(|rest| {
                let path = rest.split(['?', '#']).next().unwrap_or(rest);
                let config = config();
                ui_files::load(&config.source, path, &config.config_json)
            });
            Some(match file {
                Some(file) => bytes_response(200, file.mime, file.body),
                None => bytes_response(404, "text/plain", b"not found".to_vec()),
            })
        }
    }
}

/// Parse a command, hand it to the UI thread and answer `204 No Content` right away.
/// Whether the sender may send commands at all is checked on the UI thread.
fn command_response(sender: Option<i32>, url: &str) -> ResourceHandler {
    let Some(command) = protocol::parse_command(url) else {
        tracing::warn!(url, "unknown bmcp command");
        return bytes_response(400, "text/plain", b"unknown command".to_vec());
    };
    if let Some(sender) = sender {
        crate::bridge::post_to_ui(Box::new(move || {
            crate::chrome::handle_command(sender, command)
        }));
    }
    bytes_response(204, "text/plain", Vec::new())
}

fn bytes_response(status: i32, mime: &'static str, body: Vec<u8>) -> ResourceHandler {
    StaticResource::new(
        status,
        mime,
        Arc::new(body),
        chrome_origin(),
        Arc::new(Mutex::new(0)),
    )
}

// Serves a byte buffer. Our own handler instead of cef-rs' `StreamResourceHandler`: its
// `CefStringMultimap` header map is cloned shallowly and cleared on drop, so extra headers
// (needed for CORS) silently disappeared.
wrap_resource_handler! {
    struct StaticResource {
        status: i32,
        mime: &'static str,
        body: Arc<Vec<u8>>,
        allow_origin: String,
        offset: Arc<Mutex<usize>>,
    }

    impl ResourceHandler {
        fn open(
            &self,
            _request: Option<&mut Request>,
            handle_request: Option<&mut i32>,
            _callback: Option<&mut Callback>,
        ) -> i32 {
            if let Some(handle_request) = handle_request {
                *handle_request = 1; // answered right away
            }
            1
        }

        fn response_headers(
            &self,
            response: Option<&mut Response>,
            response_length: Option<&mut i64>,
            _redirect_url: Option<&mut CefString>,
        ) {
            if let Some(response) = response {
                response.set_status(self.status);
                response.set_mime_type(Some(&CefString::from(self.mime)));
                response.set_charset(Some(&CefString::from("utf-8")));
                let header = |name: &str, value: &str| {
                    let (name, value) = (CefString::from(name), CefString::from(value));
                    response.set_header_by_name(Some(&name), Some(&value), 1);
                };
                header("Cache-Control", "no-store");
                // UI pages (origin bmcp://ui) fetch bmcp://cmd/..., which is cross-origin.
                header("Access-Control-Allow-Origin", &self.allow_origin);
                // Our pages talk to the API with full rights: never let a web page frame
                // them (clickjacking).
                header("Content-Security-Policy", "frame-ancestors 'none'");
                header("X-Frame-Options", "DENY");
            }
            if let Some(length) = response_length {
                *length = self.body.len() as i64;
            }
        }

        #[allow(clippy::not_unsafe_ptr_arg_deref)]
        fn read(
            &self,
            data_out: *mut u8,
            bytes_to_read: i32,
            bytes_read: Option<&mut i32>,
            _callback: Option<&mut ResourceReadCallback>,
        ) -> i32 {
            let Some(bytes_read) = bytes_read else { return 0 };
            // `offset` = how many bytes were already handed to CEF.
            let mut offset = self.offset.lock().unwrap_or_else(|e| e.into_inner());
            let rest = &self.body[*offset..];
            let n = rest.len().min(usize::try_from(bytes_to_read).unwrap_or(0));
            if n == 0 || data_out.is_null() {
                *bytes_read = 0;
                return 0; // all sent
            }
            // SAFETY: CEF guarantees `data_out` has room for `bytes_to_read` bytes.
            unsafe { std::ptr::copy_nonoverlapping(rest.as_ptr(), data_out, n) };
            *offset += n;
            *bytes_read = n as i32;
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_views_may_only_show_the_new_tab_page() {
        assert!(is_allowed_in_content("bmcp://ui/newtab.html"));
        assert!(is_allowed_in_content("bmcp://ui/newtab.html?x=1"));
        assert!(!is_allowed_in_content("bmcp://ui/index.html"));
        assert!(!is_allowed_in_content("bmcp://ui/palette.html"));
        assert!(!is_allowed_in_content("bmcp://cmd/palette"));
        assert!(is_allowed_in_content("https://example.com/"));
    }

    #[test]
    fn only_our_pages_may_reach_localhost() {
        let loopback = 1 << 27;
        assert!(allow_permission("bmcp://ui", loopback));
        assert!(allow_permission("bmcp://ui/", loopback | (1 << 26)));
        assert!(!allow_permission("https://evil.test", loopback));
        // Our pages get nothing else (camera = 4) without a real prompt.
        assert!(!allow_permission("bmcp://ui", loopback | 4));
        assert!(!allow_permission("bmcp://ui", 0));
    }
}
