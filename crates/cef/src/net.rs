//! The adblock / site-policy gate in CEF's network stack (spec: `crates/adblock/README.md`,
//! "CEF hook").
//!
//! * `RequestHandler::GetResourceRequestHandler` (content browsers) and
//!   `RequestContextHandler::GetResourceRequestHandler` (requests without a browser, e.g.
//!   service workers) return a [`GateRequestHandler`].
//! * `OnBeforeResourceLoad` (IO thread) asks the gate: allow → `RV_CONTINUE`, block →
//!   `RV_CANCEL` (the request fails with `ERR_ABORTED`), redirect → continue, and
//!   `GetResourceHandler` then answers with the replacement body (a `data:` URL from the
//!   filter list's resources) instead of loading it.
//! * `OnResourceLoadComplete` reports the received bytes for `adblock.stats`.
//! * Site policies `js: false` / `images: false` become CEF content settings of the page's
//!   site before its navigation commits ([`apply_content_settings`]), so even inline scripts
//!   of the first document are blocked.
//!
//! Never blocks or awaits: `on_request` is synchronous and lock-free on the fast path.

use std::sync::{Arc, Mutex, OnceLock};

use bmcp_adblock::{GateAction, RequestInfo, ResourceGate, ResourceType};
use bmcp_core::TabId;
use cef::*;

static GATE: OnceLock<Arc<dyn ResourceGate>> = OnceLock::new();

/// Install the gate (once, before the first browser exists).
pub(crate) fn set_gate(gate: Arc<dyn ResourceGate>) {
    let _ = GATE.set(gate);
}

/// A handler for one request, or `None` when there is no gate.
pub(crate) fn request_handler() -> Option<ResourceRequestHandler> {
    let gate = GATE.get()?.clone();
    Some(GateRequestHandler::new(gate, Arc::new(Mutex::new(None))))
}

/// CEF's `cef_resource_type_t` as its number (what `ResourceType::from_cef` expects).
fn resource_type_number(request: &Request) -> i32 {
    sys::cef_resource_type_t::from(request.resource_type()) as i32
}

/// Body and MIME type of a `data:` URL (`data:text/javascript;base64,AAAA` or
/// `data:text/plain,hello%20world`).
pub(crate) fn decode_data_url(url: &str) -> Option<(String, Vec<u8>)> {
    let rest = url.strip_prefix("data:")?;
    let (meta, data) = rest.split_once(',')?;
    let base64 = meta.ends_with(";base64");
    let mime = meta
        .trim_end_matches(";base64")
        .split(';')
        .next()
        .unwrap_or("");
    let mime = if mime.is_empty() { "text/plain" } else { mime };
    let body = if base64 {
        decode_base64(data)?
    } else {
        percent_decode(data)
    };
    Some((mime.to_string(), body))
}

fn decode_base64(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let mut buffer = 0u32;
    let mut bits = 0;
    for c in text
        .bytes()
        .filter(|c| !c.is_ascii_whitespace() && *c != b'=')
    {
        let value = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => return None,
        };
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    Some(out)
}

fn percent_decode(text: &str) -> Vec<u8> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Some(byte) = std::str::from_utf8(&bytes[i + 1..i + 3])
                .ok()
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(byte);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    out
}

wrap_resource_request_handler! {
    // One per request: remembers a redirect decision between `OnBeforeResourceLoad` and
    // `GetResourceHandler`.
    pub(crate) struct GateRequestHandler {
        gate: Arc<dyn ResourceGate>,
        redirect: Arc<Mutex<Option<String>>>,
    }

    impl ResourceRequestHandler {
        fn on_before_resource_load(
            &self,
            browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            request: Option<&mut Request>,
            _callback: Option<&mut Callback>,
        ) -> ReturnValue {
            let Some(request) = request else { return ReturnValue::CONTINUE };
            let url = CefString::from(&request.url()).to_string();
            if !url.starts_with("http") && !url.starts_with("ws") {
                return ReturnValue::CONTINUE; // bmcp://, data:, blob:, chrome:… are never filtered
            }
            // The top-level page that made the request (CefFrame is usable on any thread).
            let source = browser
                .as_ref()
                .and_then(|b| b.main_frame())
                .map(|f| CefString::from(&f.url()).to_string())
                .filter(|u| !u.is_empty())
                .unwrap_or_else(|| CefString::from(&request.referrer_url()).to_string());
            let info = RequestInfo {
                tab: browser.as_ref().map(|b| TabId(b.identifier())),
                url: &url,
                source_url: &source,
                resource_type: ResourceType::from_cef(resource_type_number(request)),
            };
            let decision = self.gate.on_request(&info);
            match decision.action {
                GateAction::Allow => ReturnValue::CONTINUE,
                GateAction::Block => ReturnValue::CANCEL,
                GateAction::Redirect => {
                    *self.redirect.lock().unwrap_or_else(|e| e.into_inner()) = decision.redirect;
                    ReturnValue::CONTINUE
                }
            }
        }

        fn resource_handler(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _request: Option<&mut Request>,
        ) -> Option<ResourceHandler> {
            let redirect = self.redirect.lock().unwrap_or_else(|e| e.into_inner()).take()?;
            let (mime, body) = decode_data_url(&redirect)?;
            Some(crate::scheme::data_response(mime, body))
        }

        fn on_resource_load_complete(
            &self,
            browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _request: Option<&mut Request>,
            _response: Option<&mut Response>,
            status: UrlrequestStatus,
            received_content_length: i64,
        ) {
            if status == UrlrequestStatus::SUCCESS && received_content_length > 0 {
                let tab = browser.map(|b| TabId(b.identifier()));
                self.gate.on_complete(tab, received_content_length as u64);
            }
        }
    }
}

/// Before a main-frame navigation to `url` commits: turn the site policy's `js` / `images`
/// into content settings of that site on the tab's request context. UI thread.
pub(crate) fn apply_content_settings(browser: &Browser, url: &str) {
    let Some(gate) = GATE.get() else { return };
    if !url.starts_with("http") {
        return;
    }
    let Some((js, images)) = gate.content_settings(url) else {
        return;
    };
    let Some(context) = browser.host().and_then(|h| h.request_context()) else {
        return;
    };
    let site = CefString::from(url);
    for (kind, allowed) in [
        (ContentSettingTypes::JAVASCRIPT, js),
        (ContentSettingTypes::IMAGES, images),
    ] {
        let wanted = if allowed {
            ContentSettingValues::DEFAULT
        } else {
            ContentSettingValues::BLOCK
        };
        if context.content_setting(Some(&site), Some(&site), kind) != wanted {
            context.set_content_setting(Some(&site), Some(&site), kind, wanted);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_urls() {
        assert_eq!(
            decode_data_url("data:text/javascript;base64,KGZ1bmN0aW9uKCl7fSkoKTs="),
            Some(("text/javascript".into(), b"(function(){})();".to_vec()))
        );
        assert_eq!(
            decode_data_url("data:,a%20b"),
            Some(("text/plain".into(), b"a b".to_vec()))
        );
        assert_eq!(
            decode_data_url("data:image/gif;base64,R0lGODlhAQABAAAAACw="),
            Some((
                "image/gif".into(),
                vec![71, 73, 70, 56, 57, 97, 1, 0, 1, 0, 0, 0, 0, 44]
            ))
        );
        assert_eq!(decode_data_url("https://x"), None);
        assert_eq!(decode_data_url("data:;base64,@@"), None);
    }

    /// The README's mapping table, checked against CEF's own enum values.
    #[test]
    fn resource_type_numbers_match_cef() {
        use sys::cef_resource_type_t as rt;
        let table = [
            (rt::RT_MAIN_FRAME, ResourceType::MainFrame),
            (rt::RT_SUB_FRAME, ResourceType::SubFrame),
            (rt::RT_STYLESHEET, ResourceType::Stylesheet),
            (rt::RT_SCRIPT, ResourceType::Script),
            (rt::RT_IMAGE, ResourceType::Image),
            (rt::RT_FONT_RESOURCE, ResourceType::Font),
            (rt::RT_SUB_RESOURCE, ResourceType::Other),
            (rt::RT_OBJECT, ResourceType::Object),
            (rt::RT_MEDIA, ResourceType::Media),
            (rt::RT_WORKER, ResourceType::Worker),
            (rt::RT_SHARED_WORKER, ResourceType::Worker),
            (rt::RT_PREFETCH, ResourceType::Prefetch),
            (rt::RT_FAVICON, ResourceType::Favicon),
            (rt::RT_XHR, ResourceType::Xhr),
            (rt::RT_PING, ResourceType::Ping),
            (rt::RT_SERVICE_WORKER, ResourceType::Worker),
            (rt::RT_CSP_REPORT, ResourceType::CspReport),
            (rt::RT_PLUGIN_RESOURCE, ResourceType::Object),
            (
                rt::RT_NAVIGATION_PRELOAD_MAIN_FRAME,
                ResourceType::MainFrame,
            ),
            (rt::RT_NAVIGATION_PRELOAD_SUB_FRAME, ResourceType::SubFrame),
        ];
        for (cef_type, ours) in table {
            assert_eq!(
                ResourceType::from_cef(cef_type as i32),
                ours,
                "{cef_type:?}"
            );
        }
    }
}
