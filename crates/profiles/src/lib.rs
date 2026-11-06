//! BrowserMCP profiles: each profile is its own cookie jar + cache (`data/profiles/<id>/`),
//! with an optional proxy, Accept-Language and fingerprint identity. Also the cookie,
//! site-permission and download-folder policies the CEF crate needs.
//!
//! No CEF dependency: the browser side is the [`ProfileBackend`] trait (implemented by
//! `bmcp_cef::CefBackend`) and the [`TabPreparer`] hook; everything else is plain Rust.
//!
//! Proxy routing, forwarders, WebRTC/language preferences, identities and the worker patch
//! all come from bmcp-identity (one source of truth); this crate puts them together per
//! profile and hands them to the browser.
//!
//! | Module | What |
//! |---|---|
//! | [`service`] | `profiles.json`, applying a profile (proxy pref from its bmcp-identity routing) |
//! | [`prefs`] | the Chromium preferences of a profile (proxy, WebRTC, Accept-Language) |
//! | [`cookies`] | [`CookieInfo`], CEF time conversion, `pending_cookies` → cookie jar |
//! | [`permissions`] | `permissions.json` rules; default deny |
//! | [`downloads`] | download folder per run, safe unique file names |
//! | [`hooks`] | identity auto-apply on new tabs, worker patching (shared auto-attach) |
//! | [`rows`] | isolated mass-run rows: leased proxy per row, fitted identity |
//! | [`commands`] | `profile.list/create/delete`, `cookies.get/set/delete/apply_pending`, `permissions.set/list` |
//!
//! Wiring (the app): [`install`] puts a [`ProfilesService`] and a [`PermissionStore`] into the
//! registry, connects the browser with [`ProfilesService::set_backend`], starts [`watch`] and
//! calls [`apply_stored`] once at startup.

pub mod backend;
pub mod commands;
pub mod cookies;
pub mod downloads;
pub mod hooks;
pub mod permissions;
pub mod prefs;
pub mod rows;
pub mod service;
#[cfg(test)]
mod tests;

use std::sync::Arc;

use bmcp_core::{Ctx, EventBus, Registry};
use tokio::sync::broadcast::error::RecvError;

pub use backend::{CookieTarget, OpenProfile, PermissionPrompt, ProfileBackend, ProfileSettings};
pub use cookies::CookieInfo;
pub use hooks::{IdentityPreparer, TabPreparer};
pub use permissions::{PermissionKind, PermissionStore};
pub use prefs::Prefs;
pub use rows::{RowProfiles, RowRequest, RowSetup};
pub use service::{ProfileConfig, ProfilesService};

/// Register the commands (state comes from [`install`]).
pub fn register(registry: &mut Registry) {
    registry
        .add::<commands::List>()
        .add::<commands::Create>()
        .add::<commands::Update>()
        .add::<commands::Delete>()
        .add::<commands::CookiesGet>()
        .add::<commands::CookiesSet>()
        .add::<commands::CookiesDelete>()
        .add::<commands::CookiesApplyPending>()
        .add::<commands::PermissionsSet>()
        .add::<commands::PermissionsList>()
        .add::<commands::PermissionsPending>()
        .add::<commands::PermissionsAnswer>();
}

/// Give the commands their state: the profile store under `data_root` and the permission
/// rules shared with the browser. Call after `bmcp_identity::install` (the profiles use its
/// services).
pub fn install(
    registry: &Registry,
    data_root: &std::path::Path,
    launch_profile: &str,
    permissions: PermissionStore,
) -> Arc<ProfilesService> {
    registry.extensions().insert(permissions);
    let service = registry
        .extensions()
        .insert(ProfilesService::new(data_root, launch_profile));
    match registry.extensions().get::<bmcp_identity::Services>() {
        Some(identities) => service.set_identities(identities),
        None => tracing::warn!("bmcp_profiles::install before bmcp_identity::install"),
    }
    service
}

/// Apply every profile that has settings (profiles.json, proxy routing, assigned identity):
/// start forwarders, set prefs. Called once at startup, after the backend is connected.
/// Errors are logged, not fatal.
pub async fn apply_stored(ctx: &Ctx) {
    let Ok(service) = ProfilesService::from_ctx(ctx) else {
        return;
    };
    let ids = match service.known_profiles() {
        Ok(ids) => ids,
        Err(err) => {
            tracing::warn!(%err, "profile settings unreadable");
            return;
        }
    };
    for id in ids {
        if let Err(err) = service.apply(&id).await {
            tracing::warn!(profile = id, %err, "profile settings not applied");
        }
    }
}

/// Keep the browser in step with bmcp-identity: on `proxy.routes_changed`
/// ([`bmcp_identity::ROUTES_CHANGED`], e.g. `proxy.rules_set`) and `identity.assigned`
/// ([`bmcp_identity::IDENTITY_ASSIGNED`]) re-apply that profile's preferences — live, through
/// [`ProfileBackend::configure`] (`SetPreference` on its request contexts). Subscribes now;
/// call before [`apply_stored`] so no change is missed. Runs until the bus closes.
pub fn watch(service: Arc<ProfilesService>, events: &EventBus) -> tokio::task::JoinHandle<()> {
    let mut rx = events.subscribe();
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(event)
                    if event.topic == bmcp_identity::ROUTES_CHANGED
                        || event.topic == bmcp_identity::IDENTITY_ASSIGNED =>
                {
                    let Some(profile) = event.data["profile"].as_str() else {
                        continue;
                    };
                    match service.apply(profile).await {
                        Ok(prefs) => tracing::debug!(
                            profile,
                            topic = event.topic,
                            proxy = prefs.proxy_mode(),
                            "profile preferences re-applied"
                        ),
                        Err(err) => {
                            tracing::warn!(profile, %err, "profile preferences not re-applied")
                        }
                    }
                }
                Ok(_) => {}
                Err(RecvError::Lagged(lost)) => {
                    // Some change may be lost: apply everything again.
                    tracing::warn!(lost, "profile watcher lost events; re-applying all");
                    for id in service.known_profiles().unwrap_or_default() {
                        let _ = service.apply(&id).await;
                    }
                }
                Err(RecvError::Closed) => break,
            }
        }
    })
}
