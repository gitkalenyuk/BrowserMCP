//! BrowserMCP profiles: each profile is its own cookie jar + cache (`data/profiles/<id>/`),
//! with an optional proxy, Accept-Language and fingerprint identity. Also the cookie,
//! site-permission and download-folder policies the CEF crate needs.
//!
//! No CEF dependency: the browser side is the [`ProfileBackend`] trait (implemented by
//! `bmcp_cef::CefBackend`) and the [`TabPreparer`] hook; everything else is plain Rust.
//!
//! | Module | What |
//! |---|---|
//! | [`service`] | `profiles.json`, proxy plan (local forwarder from bmcp-identity), applying settings |
//! | [`prefs`] | the Chromium preferences of a profile (proxy, WebRTC, Accept-Language) |
//! | [`cookies`] | [`CookieInfo`], CEF time conversion, `pending_cookies` → cookie jar |
//! | [`permissions`] | `permissions.json` rules; default deny |
//! | [`downloads`] | download folder per run, safe unique file names |
//! | [`hooks`] | identity auto-apply on new tabs (+ worker overrides) |
//! | [`commands`] | `profile.list/create/delete`, `cookies.get/set/delete/apply_pending`, `permissions.set/list` |
//!
//! Wiring (the app): [`install`] puts a [`ProfilesService`] and a [`PermissionStore`] into the
//! registry, connects the browser with [`ProfilesService::set_backend`] and calls
//! [`apply_stored`] once at startup.

pub mod backend;
pub mod commands;
pub mod cookies;
pub mod downloads;
pub mod hooks;
pub mod permissions;
pub mod prefs;
pub mod service;
#[cfg(test)]
mod tests;

use bmcp_core::{Ctx, Registry};

pub use backend::{CookieTarget, OpenProfile, PermissionPrompt, ProfileBackend, ProfileSettings};
pub use cookies::CookieInfo;
pub use hooks::{IdentityPreparer, TabPreparer, WorkerOverrides};
pub use permissions::{PermissionKind, PermissionStore};
pub use prefs::Prefs;
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
/// rules shared with the browser.
pub fn install(
    registry: &Registry,
    data_root: &std::path::Path,
    launch_profile: &str,
    permissions: PermissionStore,
) -> std::sync::Arc<ProfilesService> {
    registry.extensions().insert(permissions);
    registry
        .extensions()
        .insert(ProfilesService::new(data_root, launch_profile))
}

/// Apply every stored profile (start forwarders, set prefs). Called once at startup, after
/// the backend is connected. Errors are logged, not fatal.
pub async fn apply_stored(ctx: &Ctx) {
    let Ok(service) = ProfilesService::from_ctx(ctx) else {
        return;
    };
    let ids: Vec<String> = match service.store.read() {
        Ok(file) => file.profiles.keys().cloned().collect(),
        Err(err) => {
            tracing::warn!(%err, "profiles.json unreadable");
            return;
        }
    };
    for id in ids {
        let lang = ctx
            .extensions
            .get::<bmcp_identity::Services>()
            .and_then(|s| s.identities.read().ok())
            .and_then(|f| {
                let identity = f.profiles.get(&id)?;
                f.identities
                    .get(identity)
                    .map(|i| i.accept_language.clone())
            });
        if let Err(err) = service.apply(&id, lang).await {
            tracing::warn!(profile = id, %err, "profile settings not applied");
        }
    }
}
