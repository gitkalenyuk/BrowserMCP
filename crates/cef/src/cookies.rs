//! Cookies of a profile through its `CefCookieManager` (`cookies.*` commands, and the
//! `pending_cookies` of `import.chrome`).
//!
//! CEF reports results through callbacks: a cookie visitor is called once per cookie and
//! released after the last one (or right away when there are none), so the collected cookies
//! are sent back when the visitor is dropped ([`Done`]). `SetCookie` answers through a
//! `SetCookieCallback` per cookie.

use std::sync::{Arc, Mutex};

use bmcp_core::{BackendError, BackendResult};
use bmcp_profiles::CookieTarget;
use bmcp_profiles::cookies::{
    CookieInfo, cef_cookie_domain, cef_to_unix_ms, cookie_url, domain_matches, unix_ms_to_cef,
};
use cef::*;
use tokio::sync::oneshot;

use crate::bridge::run_on_ui;
use crate::profiles::{ProfileKey, context_or_global};

/// The cookie manager of `target`'s profile. UI thread.
fn manager(target: &CookieTarget) -> BackendResult<CookieManager> {
    let key = ProfileKey::from_options(target.profile.as_deref(), target.incognito)?;
    context_or_global(&key)
        .and_then(|c| c.cookie_manager(None))
        .ok_or_else(|| BackendError::Cdp("no cookie manager for this profile".into()))
}

fn to_info(c: &Cookie) -> CookieInfo {
    let text = |s: &CefString| s.to_string();
    let same_site = if c.same_site == CookieSameSite::NO_RESTRICTION {
        "no_restriction"
    } else if c.same_site == CookieSameSite::LAX_MODE {
        "lax"
    } else if c.same_site == CookieSameSite::STRICT_MODE {
        "strict"
    } else {
        "unspecified"
    };
    CookieInfo {
        name: text(&c.name),
        value: text(&c.value),
        domain: text(&c.domain),
        path: text(&c.path),
        secure: c.secure == 1,
        http_only: c.httponly == 1,
        expires: if c.has_expires == 1 {
            cef_to_unix_ms(c.expires.val)
        } else {
            None
        },
        same_site: same_site.to_string(),
    }
}

fn to_cef(c: &CookieInfo) -> Cookie {
    let same_site = match c.same_site.as_str() {
        "no_restriction" => CookieSameSite::NO_RESTRICTION,
        "lax" => CookieSameSite::LAX_MODE,
        "strict" => CookieSameSite::STRICT_MODE,
        _ => CookieSameSite::UNSPECIFIED,
    };
    Cookie {
        name: CefString::from(c.name.as_str()),
        value: CefString::from(c.value.as_str()),
        domain: CefString::from(cef_cookie_domain(&c.domain)),
        path: CefString::from(c.path.as_str()),
        secure: i32::from(c.secure),
        httponly: i32::from(c.http_only),
        has_expires: i32::from(c.expires.is_some()),
        expires: Basetime {
            val: c.expires.map_or(0, unix_ms_to_cef),
        },
        same_site,
        priority: CookiePriority::MEDIUM,
        ..Default::default()
    }
}

/// Sends the collected cookies when the last reference (CEF's visitor) goes away.
struct Done {
    tx: Mutex<Option<oneshot::Sender<Vec<CookieInfo>>>>,
    items: Mutex<Vec<CookieInfo>>,
}

impl Drop for Done {
    fn drop(&mut self) {
        let items = std::mem::take(&mut *self.items.lock().unwrap_or_else(|e| e.into_inner()));
        if let Some(tx) = self.tx.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = tx.send(items);
        }
    }
}

wrap_cookie_visitor! {
    struct Collector {
        done: Arc<Done>,
        domain: Option<String>,
        name: Option<String>,
        delete: bool,
    }

    impl CookieVisitor {
        fn visit(
            &self,
            cookie: Option<&Cookie>,
            _count: ::std::os::raw::c_int,
            _total: ::std::os::raw::c_int,
            delete_cookie: Option<&mut ::std::os::raw::c_int>,
        ) -> ::std::os::raw::c_int {
            let Some(cookie) = cookie else { return 1 };
            let info = to_info(cookie);
            let wanted = domain_matches(&info.domain, self.domain.as_deref().unwrap_or(""))
                && self.name.as_ref().is_none_or(|n| *n == info.name);
            if wanted {
                if self.delete && let Some(delete) = delete_cookie {
                    *delete = 1;
                }
                self.done.items.lock().unwrap_or_else(|e| e.into_inner()).push(info);
            }
            1 // keep visiting
        }
    }
}

/// Visit every cookie of `target`; matching ones are returned (and deleted with `delete`).
async fn visit(
    target: CookieTarget,
    domain: Option<String>,
    name: Option<String>,
    delete: bool,
) -> BackendResult<Vec<CookieInfo>> {
    let (tx, rx) = oneshot::channel();
    run_on_ui(move || {
        let manager = manager(&target)?;
        let done = Arc::new(Done {
            tx: Mutex::new(Some(tx)),
            items: Mutex::new(Vec::new()),
        });
        let mut collector = Collector::new(done, domain, name, delete);
        if manager.visit_all_cookies(Some(&mut collector)) != 1 {
            return Err(BackendError::Cdp("cookies cannot be read right now".into()));
        }
        Ok(())
    })
    .await??;
    rx.await.map_err(|_| BackendError::ShuttingDown)
}

pub(crate) async fn get(
    target: CookieTarget,
    domain: Option<String>,
) -> BackendResult<Vec<CookieInfo>> {
    visit(target, domain, None, false).await
}

pub(crate) async fn delete(
    target: CookieTarget,
    domain: Option<String>,
    name: Option<String>,
) -> BackendResult<usize> {
    Ok(visit(target, domain, name, true).await?.len())
}

wrap_set_cookie_callback! {
    struct SetDone {
        tx: Arc<Mutex<Option<oneshot::Sender<bool>>>>,
    }

    impl SetCookieCallback {
        fn on_complete(&self, success: ::std::os::raw::c_int) {
            if let Some(tx) = self.tx.lock().unwrap_or_else(|e| e.into_inner()).take() {
                let _ = tx.send(success == 1);
            }
        }
    }
}

pub(crate) async fn set(
    target: CookieTarget,
    cookies: Vec<CookieInfo>,
) -> BackendResult<Vec<bool>> {
    let receivers = run_on_ui(move || {
        let manager = manager(&target)?;
        let mut receivers = Vec::with_capacity(cookies.len());
        for cookie in &cookies {
            let (tx, rx) = oneshot::channel();
            let mut callback = SetDone::new(Arc::new(Mutex::new(Some(tx))));
            let url = CefString::from(cookie_url(cookie).as_str());
            let ok = manager.set_cookie(Some(&url), Some(&to_cef(cookie)), Some(&mut callback));
            // 0 = refused at once (invalid URL or cookie): the callback never runs.
            receivers.push((ok == 1).then_some(rx));
        }
        Ok::<_, BackendError>(receivers)
    })
    .await??;
    let mut results = Vec::with_capacity(receivers.len());
    for rx in receivers {
        results.push(match rx {
            Some(rx) => rx.await.unwrap_or(false),
            None => false,
        });
    }
    Ok(results)
}
