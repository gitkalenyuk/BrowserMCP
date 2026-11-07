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
    /// `Content-Security-Policy` of every HTML page, see [`content_security_policy`].
    csp: String,
}

static CONFIG: OnceLock<Config> = OnceLock::new();

fn config() -> &'static Config {
    CONFIG.get_or_init(|| Config {
        source: Source::Embedded,
        config_json: "{}".into(),
        dev_url: None,
        api_ws: None,
        csp: content_security_policy("{}", None),
    })
}

/// Remember the UI options (call before CEF starts).
pub(crate) fn configure(ui: &crate::UiOptions) {
    let config_json = serde_json::json!({ "ws": ui.api_ws }).to_string();
    let csp = content_security_policy(&config_json, ui.api_ws.as_deref());
    let _ = CONFIG.set(Config {
        source: ui.dir.clone().map_or(Source::Embedded, Source::Folder),
        config_json,
        dev_url: ui
            .dev_url
            .as_ref()
            .map(|u| u.trim_end_matches('/').to_owned()),
        api_ws: ui.api_ws.clone(),
        csp,
    });
}

/// Base64 of the SHA-256 of `bytes` (CSP hash sources).
fn sha256_base64(bytes: &[u8]) -> String {
    use base64::Engine;
    use sha2::Digest;
    base64::engine::general_purpose::STANDARD.encode(sha2::Sha256::digest(bytes))
}

/// The CSP of our pages: they talk to the API with full rights, so they may run only our
/// own scripts (files from `bmcp://ui` + the one injected config script, by hash — no other
/// inline script, no `javascript:` URLs, no eval) and connect only to our own origin, the
/// `bmcp://cmd` channel and the local API port (no foreign WebSocket endpoints).
pub(crate) fn content_security_policy(config_json: &str, api_ws: Option<&str>) -> String {
    let hash = sha256_base64(crate::ui_files::config_script(config_json).as_bytes());
    // `ws://127.0.0.1:47800/v1/ws` → `127.0.0.1:47800`.
    let api = api_ws
        .and_then(|ws| ws.split_once("://"))
        .map(|(_, rest)| rest.split('/').next().unwrap_or(rest).to_string())
        .filter(|host| !host.is_empty());
    let connect = match api {
        Some(host) => format!("'self' bmcp://cmd ws://{host} http://{host}"),
        None => "'self' bmcp://cmd".to_string(),
    };
    format!(
        "default-src 'self'; script-src 'self' 'sha256-{hash}'; \
         style-src 'self' 'unsafe-inline'; img-src 'self' data: https: http:; \
         font-src 'self' data:; connect-src {connect}; object-src 'none'; base-uri 'none'; \
         frame-ancestors 'none'"
    )
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

/// `url` with `window=<key>` added to its query (the chrome page reads its window key).
pub(crate) fn with_window_key(url: &str, key: i32) -> String {
    let sep = if url.contains('?') { '&' } else { '?' };
    format!("{url}{sep}window={key}")
}

/// May a chrome view (UI / palette) show this URL?
pub(crate) fn is_chrome_url(url: &str) -> bool {
    url.starts_with(UI_PREFIX)
        || config()
            .dev_url
            .as_deref()
            .is_some_and(|dev| url.starts_with(&format!("{dev}/")))
}

/// The new tab page, exactly (no query, no fragment: the UI reads its API endpoint from the
/// query in dev mode, so `newtab.html?ws=wss://evil` must never load in a tab).
pub(crate) fn newtab_url() -> String {
    format!("{UI_PREFIX}{NEWTAB_PAGE}")
}

/// May a web page (content view) show this URL? Of our own pages only the exact new tab
/// page: the chrome and palette pages have no business inside a tab.
pub(crate) fn is_allowed_in_content(url: &str) -> bool {
    if url.starts_with("bmcp:") {
        // The script library page may also be opened in a tab (exactly, no query).
        return url == newtab_url()
            || url == format!("{UI_PREFIX}library.html")
            || is_settings_page(url);
    }
    true
}

/// The settings page, exactly, or with a user-guide anchor
/// (`bmcp://ui/settings.html#guide/<chapter>[/<section>]`, ui/src/lib/settings/open.ts).
/// Never with a query, and no other fragment: the page reads nothing else from its URL.
fn is_settings_page(url: &str) -> bool {
    let Some(rest) = url
        .strip_prefix(UI_PREFIX)
        .and_then(|u| u.strip_prefix("settings.html"))
    else {
        return false;
    };
    rest.is_empty() || rest.strip_prefix("#guide/").is_some_and(is_guide_anchor)
}

/// `chapter` or `chapter/section`, each 1–40 of `a-z 0-9 -` (the same rule as the page's
/// `parseGuideHash`).
fn is_guide_anchor(anchor: &str) -> bool {
    let id = |p: &str| {
        (1..=40).contains(&p.len())
            && p.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    };
    (1..=2).contains(&anchor.split('/').count()) && anchor.split('/').all(id)
}

/// `cef_transition_type_t` bits used by [`content_navigation_allowed`].
const TT_SOURCE_MASK: u32 = 0xFF;
const TT_RELOAD: u32 = 8;
const TT_FORWARD_BACK_FLAG: u32 = 0x0100_0000;
/// "Loaded a URL directly via CreateBrowser, LoadURL or LoadRequest" = browser-initiated.
const TT_DIRECT_LOAD_FLAG: u32 = 0x0200_0000;
const TT_IS_REDIRECT_MASK: u32 = 0xC000_0000;

/// May a content view navigate to `target`? (`current` = the frame's URL before it.)
///
/// Web URLs: always. `bmcp://`: only the exact new tab page, only in the main frame, never
/// through a redirect, and only when the browser itself started the load (new tab, `nav.goto`,
/// back/forward, reload) or the tab already shows one of our pages — a page on some web
/// origin cannot send a tab to `bmcp://` (link, `location = …`, form, redirect).
pub(crate) fn content_navigation_allowed(
    target: &str,
    current: &str,
    transition: u32,
    is_redirect: bool,
    is_main_frame: bool,
) -> bool {
    if !target.starts_with("bmcp:") {
        return true;
    }
    if !is_allowed_in_content(target) || !is_main_frame {
        return false;
    }
    if is_redirect || transition & TT_IS_REDIRECT_MASK != 0 {
        return false;
    }
    let browser_initiated = transition & (TT_DIRECT_LOAD_FLAG | TT_FORWARD_BACK_FLAG) != 0
        || transition & TT_SOURCE_MASK == TT_RELOAD;
    let current_is_ours =
        current.is_empty() || current == "about:blank" || current.starts_with(UI_PREFIX);
    browser_initiated || current_is_ours
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
    // DISPLAY_ISOLATED: only `bmcp://` pages may link to / frame / navigate to `bmcp://`
    // (browser-initiated loads like a new tab or `nav.goto` still work).
    let options = SchemeOptions::STANDARD.get_raw()
        | SchemeOptions::SECURE.get_raw()
        | SchemeOptions::CORS_ENABLED.get_raw()
        | SchemeOptions::FETCH_ENABLED.get_raw()
        | SchemeOptions::DISPLAY_ISOLATED.get_raw();
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

/// Install the `bmcp://` handler on a profile's own request context (non-global contexts
/// do not see the global registration).
pub(crate) fn register_on(context: &RequestContext) {
    let mut factory = BmcpSchemeFactory::new();
    let ok = context.register_scheme_handler_factory(
        Some(&CefString::from(SCHEME)),
        None,
        Some(&mut factory),
    );
    if ok != 1 {
        tracing::error!("could not register bmcp:// on a profile context");
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
    let mut headers = vec![
        ("Cache-Control".to_string(), "no-store".to_string()),
        // UI pages (origin bmcp://ui) fetch bmcp://cmd/..., which is cross-origin.
        ("Access-Control-Allow-Origin".to_string(), chrome_origin()),
        // Never let a web page frame our pages (clickjacking).
        ("X-Frame-Options".to_string(), "DENY".to_string()),
    ];
    // HTML pages get the full policy (see `content_security_policy`), the rest at least
    // frame-ancestors.
    let csp = if mime.starts_with("text/html") {
        config().csp.clone()
    } else {
        "frame-ancestors 'none'".to_string()
    };
    headers.push(("Content-Security-Policy".to_string(), csp));
    StaticResource::new(
        status,
        mime.to_string(),
        Arc::new(body),
        Arc::new(headers),
        Arc::new(Mutex::new(0)),
    )
}

/// A plain `200` answer with `body` (adblock redirect resources served instead of a request).
pub(crate) fn data_response(mime: String, body: Vec<u8>) -> ResourceHandler {
    let headers = vec![("Access-Control-Allow-Origin".to_string(), "*".to_string())];
    StaticResource::new(
        200,
        mime,
        Arc::new(body),
        Arc::new(headers),
        Arc::new(Mutex::new(0)),
    )
}

// Serves a byte buffer. Our own handler instead of cef-rs' `StreamResourceHandler`: its
// `CefStringMultimap` header map is cloned shallowly and cleared on drop, so extra headers
// (needed for CORS) silently disappeared.
wrap_resource_handler! {
    struct StaticResource {
        status: i32,
        mime: String,
        body: Arc<Vec<u8>>,
        headers: Arc<Vec<(String, String)>>,
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
                response.set_mime_type(Some(&CefString::from(self.mime.as_str())));
                response.set_charset(Some(&CefString::from("utf-8")));
                for (name, value) in self.headers.iter() {
                    let name = CefString::from(name.as_str());
                    let value = CefString::from(value.as_str());
                    response.set_header_by_name(Some(&name), Some(&value), 1);
                }
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
    fn window_key_goes_into_the_query() {
        assert_eq!(
            with_window_key("bmcp://ui/index.html", 7),
            "bmcp://ui/index.html?window=7"
        );
        assert_eq!(
            with_window_key("http://localhost:5173/index.html?api=ws", 7),
            "http://localhost:5173/index.html?api=ws&window=7"
        );
    }

    #[test]
    fn content_views_may_only_show_the_new_tab_page() {
        assert!(is_allowed_in_content("bmcp://ui/newtab.html"));
        assert!(!is_allowed_in_content("bmcp://ui/newtab.html?x=1"));
        assert!(!is_allowed_in_content(
            "bmcp://ui/newtab.html?api=ws&ws=wss://evil/rpc"
        ));
        assert!(!is_allowed_in_content("bmcp://ui/newtab.html#x"));
        assert!(!is_allowed_in_content("bmcp://ui/index.html"));
        assert!(!is_allowed_in_content("bmcp://ui/palette.html"));
        assert!(!is_allowed_in_content("bmcp://cmd/palette"));
        assert!(is_allowed_in_content("https://example.com/"));
    }

    #[test]
    fn the_settings_page_may_carry_a_guide_anchor_and_nothing_else() {
        assert!(is_allowed_in_content("bmcp://ui/settings.html"));
        assert!(is_allowed_in_content("bmcp://ui/settings.html#guide/tabs"));
        assert!(is_allowed_in_content(
            "bmcp://ui/settings.html#guide/traffic/limits"
        ));
        assert!(is_allowed_in_content(
            "bmcp://ui/settings.html#guide/ai-panel/modes"
        ));
        for bad in [
            "bmcp://ui/settings.html?x=1",
            "bmcp://ui/settings.html?api=ws#guide/tabs",
            "bmcp://ui/settings.html#",
            "bmcp://ui/settings.html#x",
            "bmcp://ui/settings.html#guide/",
            "bmcp://ui/settings.html#guide/Tabs",
            "bmcp://ui/settings.html#guide/a/b/c",
            "bmcp://ui/settings.html#guide/tabs/",
            "bmcp://ui/settings.html#guide//tabs",
            "bmcp://ui/settings.html#guide/tabs?x=1",
            "bmcp://ui/settings.html#guide/%3Cx%3E",
            "bmcp://ui/settings.htmlx",
            "bmcp://ui/settings.html/x",
            "bmcp://ui/index.html#guide/tabs",
        ] {
            assert!(!is_allowed_in_content(bad), "{bad}");
        }
        let long = format!("bmcp://ui/settings.html#guide/{}", "a".repeat(41));
        assert!(!is_allowed_in_content(&long));
        // The rest of the tab rules still apply: a web page cannot send a tab there.
        let anchored = "bmcp://ui/settings.html#guide/tabs";
        let (direct, link) = (TT_DIRECT_LOAD_FLAG | 1, 0);
        assert!(content_navigation_allowed(
            anchored, "", direct, false, true
        ));
        assert!(!content_navigation_allowed(
            anchored,
            "https://evil.test/",
            link,
            false,
            true
        ));
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

    #[test]
    fn web_pages_cannot_send_a_tab_to_bmcp() {
        let newtab = "bmcp://ui/newtab.html";
        let direct = TT_DIRECT_LOAD_FLAG | 1; // LoadURL / CreateBrowser (TT_EXPLICIT)
        let link = 0; // TT_LINK
        // Browser-initiated: new tab, nav.goto from a web page, back/forward, reload.
        assert!(content_navigation_allowed(newtab, "", direct, false, true));
        assert!(content_navigation_allowed(
            newtab,
            "https://a.test/",
            direct,
            false,
            true
        ));
        assert!(content_navigation_allowed(
            newtab,
            "https://a.test/",
            TT_FORWARD_BACK_FLAG,
            false,
            true
        ));
        assert!(content_navigation_allowed(
            newtab, newtab, TT_RELOAD, false, true
        ));
        // Renderer-initiated from a web origin: link, location=, redirects, iframes.
        assert!(!content_navigation_allowed(
            newtab,
            "https://evil.test/",
            link,
            false,
            true
        ));
        assert!(!content_navigation_allowed(
            newtab,
            "https://evil.test/",
            0x4000_0000, // client redirect
            false,
            true
        ));
        assert!(!content_navigation_allowed(
            newtab,
            "https://evil.test/",
            direct,
            true,
            true
        ));
        assert!(!content_navigation_allowed(
            newtab, "", direct, false, false
        ));
        // Never with a query, never other pages.
        assert!(!content_navigation_allowed(
            "bmcp://ui/newtab.html?ws=wss://evil",
            "",
            direct,
            false,
            true
        ));
        assert!(!content_navigation_allowed(
            "bmcp://ui/index.html",
            "",
            direct,
            false,
            true
        ));
        assert!(!content_navigation_allowed(
            "bmcp://cmd/palette",
            "",
            direct,
            false,
            true
        ));
        // Links from our own new tab page are fine; web URLs always.
        assert!(content_navigation_allowed(
            newtab, newtab, link, false, true
        ));
        assert!(content_navigation_allowed(
            "https://b.test/",
            "https://a.test/",
            link,
            true,
            false
        ));
    }

    #[test]
    fn csp_allows_only_our_config_script_and_api() {
        let json = r#"{"ws":"ws://127.0.0.1:47800/v1/ws"}"#;
        let csp = content_security_policy(json, Some("ws://127.0.0.1:47800/v1/ws"));
        assert!(csp.contains("default-src 'self'"));
        assert!(csp.contains("frame-ancestors 'none'"));
        assert!(csp.contains(
            "connect-src 'self' bmcp://cmd ws://127.0.0.1:47800 http://127.0.0.1:47800;"
        ));
        // The hash of exactly the injected script text.
        let script = crate::ui_files::config_script(json);
        let expected = format!("'sha256-{}'", sha256_base64(script.as_bytes()));
        assert!(
            csp.contains(&format!("script-src 'self' {expected};")),
            "{csp}"
        );
        // Known SHA-256 (of "abc"), base64.
        assert_eq!(
            sha256_base64(b"abc"),
            "ungWv48Bz+pBQUDeXa4iI7ADYaOWF3qctBD/YfIAFa0="
        );
        // Without an API port nothing but our own origin is reachable.
        let csp = content_security_policy("{}", None);
        assert!(csp.contains("connect-src 'self' bmcp://cmd;"));
    }
}
