//! The `bmcp://` scheme.
//!
//! * `bmcp://ui/...`  — our own chrome pages (embedded in the executable).
//! * `bmcp://cmd/...` — spike-S1 command channel from those pages, see
//!   [`crate::chrome::protocol`].
//!
//! A custom scheme must be registered in EVERY process (browser, renderer, GPU, ...)
//! before CEF starts, with the same options: that is what [`register_custom_schemes`]
//! does from `App::on_register_custom_schemes`. Sub-processes therefore get the small
//! [`SchemeApp`] in `execute_process`. The handler that actually serves the bytes
//! ([`register_handler`]) only lives in the browser process.

use std::sync::{Arc, Mutex};

use cef::*;

use crate::chrome::protocol::{self, CMD_PREFIX};

pub const SCHEME: &str = "bmcp";
/// Origin of our chrome pages.
pub const UI_ORIGIN: &str = "bmcp://ui";
pub const UI_URL: &str = "bmcp://ui/index.html";
pub const PALETTE_URL: &str = "bmcp://ui/palette.html";

/// Embedded chrome pages: (path under `bmcp://ui/`, MIME type, contents).
/// Placeholders for the spike; the Svelte build replaces them in Phase 2.
const UI_FILES: &[(&str, &str, &str)] = &[
    ("index.html", "text/html", include_str!("../assets/ui.html")),
    (
        "palette.html",
        "text/html",
        include_str!("../assets/palette.html"),
    ),
];

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
            let path = url
                .strip_prefix("bmcp://ui/")
                .map(|p| p.split(['?', '#']).next().unwrap_or(p))
                .map(|p| if p.is_empty() { "index.html" } else { p });
            let file = path.and_then(|p| UI_FILES.iter().find(|(name, _, _)| *name == p));
            Some(match file {
                Some((_, mime, body)) => bytes_response(200, mime, body.as_bytes()),
                None => bytes_response(404, "text/plain", b"not found"),
            })
        }
    }
}

/// Parse a command, hand it to the UI thread and answer `204 No Content` right away.
/// Whether the sender may send commands at all is checked on the UI thread.
fn command_response(sender: Option<i32>, url: &str) -> ResourceHandler {
    let Some(command) = protocol::parse_command(url) else {
        tracing::warn!(url, "unknown bmcp command");
        return bytes_response(400, "text/plain", b"unknown command");
    };
    if let Some(sender) = sender {
        crate::bridge::post_to_ui(Box::new(move || {
            crate::chrome::handle_command(sender, command)
        }));
    }
    bytes_response(204, "text/plain", b"")
}

fn bytes_response(status: i32, mime: &'static str, body: &'static [u8]) -> ResourceHandler {
    StaticResource::new(status, mime, body, Arc::new(Mutex::new(0)))
}

// Serves a static byte slice. Our own handler instead of cef-rs'
// `StreamResourceHandler`: its `CefStringMultimap` header map is cloned shallowly and
// cleared on drop, so extra headers (needed for CORS) silently disappeared.
wrap_resource_handler! {
    struct StaticResource {
        status: i32,
        mime: &'static str,
        body: &'static [u8],
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
                header("Access-Control-Allow-Origin", UI_ORIGIN);
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
