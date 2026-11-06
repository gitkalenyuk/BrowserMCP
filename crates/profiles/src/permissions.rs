//! Site permissions (camera, microphone, geolocation, notifications, …).
//!
//! The browser has no permission prompt UI, and an unanswered prompt waits forever, so every
//! request is answered at once: from the rules in `data/config/permissions.json`
//! (`permissions.set {origin, kind, allow}`), otherwise **denied** (except
//! `multiple_downloads`, allowed by default). A rule for origin `"*"`
//! applies to every site without its own rule.
//!
//! The CEF crate maps Chromium's permission bits to [`PermissionKind`]s with
//! [`kinds_from_prompt_bits`] / [`kinds_from_media_bits`] and asks [`PermissionStore::decide`].

use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use bmcp_core::CmdError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// One kind of permission a page can ask for.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum PermissionKind {
    Camera,
    Microphone,
    /// Screen/tab/window capture (`getDisplayMedia`).
    ScreenCapture,
    Geolocation,
    Notifications,
    Clipboard,
    Midi,
    /// Storage access (incl. top-level) for third-party frames.
    StorageAccess,
    LocalFonts,
    IdleDetection,
    WindowManagement,
    FileSystem,
    /// Several downloads from one page.
    MultipleDownloads,
    PointerLock,
    KeyboardLock,
    /// Talk to devices on the local network / localhost.
    LocalNetwork,
    /// Motion/orientation sensors.
    Sensors,
    /// Anything else (AR/VR, protocol handlers, app install, protected media id, …).
    Other,
}

impl PermissionKind {
    /// The answer when no rule exists: deny, except several downloads from one page (an
    /// automation run downloading a list of files must not hang on a prompt nobody sees).
    pub fn allowed_by_default(self) -> bool {
        self == PermissionKind::MultipleDownloads
    }
}

/// Bits of CEF's `cef_permission_request_types_t` (`OnShowPermissionPrompt`).
const PROMPT_BITS: &[(u32, PermissionKind)] = &[
    (1 << 1, PermissionKind::Camera),        // CAMERA_PAN_TILT_ZOOM
    (1 << 2, PermissionKind::Camera),        // CAMERA_STREAM
    (1 << 3, PermissionKind::ScreenCapture), // CAPTURED_SURFACE_CONTROL
    (1 << 4, PermissionKind::Clipboard),
    (1 << 5, PermissionKind::StorageAccess), // TOP_LEVEL_STORAGE_ACCESS
    (1 << 7, PermissionKind::LocalFonts),
    (1 << 8, PermissionKind::Geolocation),
    (1 << 11, PermissionKind::IdleDetection),
    (1 << 12, PermissionKind::Microphone), // MIC_STREAM
    (1 << 13, PermissionKind::Midi),       // MIDI_SYSEX
    (1 << 14, PermissionKind::MultipleDownloads),
    (1 << 15, PermissionKind::Notifications),
    (1 << 16, PermissionKind::KeyboardLock),
    (1 << 17, PermissionKind::PointerLock),
    (1 << 20, PermissionKind::StorageAccess),
    (1 << 23, PermissionKind::WindowManagement),
    (1 << 24, PermissionKind::FileSystem),
    (1 << 25, PermissionKind::LocalNetwork), // LOCAL_NETWORK_ACCESS (deprecated name)
    (1 << 26, PermissionKind::LocalNetwork),
    (1 << 27, PermissionKind::LocalNetwork), // LOOPBACK_NETWORK
    (1 << 28, PermissionKind::Sensors),
];

/// The kinds behind a CEF permission prompt bit mask (unknown bits → `Other`), without
/// duplicates, in a stable order.
pub fn kinds_from_prompt_bits(bits: u32) -> Vec<PermissionKind> {
    let mut known = 0;
    let mut kinds = Vec::new();
    for (bit, kind) in PROMPT_BITS {
        known |= bit;
        if bits & bit != 0 {
            kinds.push(*kind);
        }
    }
    if bits & !known != 0 {
        kinds.push(PermissionKind::Other);
    }
    kinds.sort();
    kinds.dedup();
    kinds
}

/// The kinds behind CEF's `cef_media_access_permission_types_t` (getUserMedia /
/// getDisplayMedia): 1 = microphone, 2 = camera, 4/8 = desktop audio/video capture.
pub fn kinds_from_media_bits(bits: u32) -> Vec<PermissionKind> {
    let mut kinds = Vec::new();
    if bits & 1 != 0 {
        kinds.push(PermissionKind::Microphone);
    }
    if bits & 2 != 0 {
        kinds.push(PermissionKind::Camera);
    }
    if bits & (4 | 8) != 0 {
        kinds.push(PermissionKind::ScreenCapture);
    }
    kinds.sort();
    kinds
}

/// `https://Example.com/` → `https://example.com` (scheme + host + port, lowercase).
pub fn normalize_origin(origin: &str) -> String {
    let origin = origin.trim();
    if origin == "*" {
        return "*".into();
    }
    let lower = origin.to_ascii_lowercase();
    // Keep only scheme://host[:port].
    match lower.find("://") {
        Some(i) => {
            let rest = &lower[i + 3..];
            let end = rest
                .find(['/', '?', '#'])
                .map_or(lower.len(), |e| i + 3 + e);
            lower[..end].to_string()
        }
        None => lower.trim_end_matches('/').to_string(),
    }
}

/// One stored rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PermissionRule {
    /// `https://example.com`, or `*` for every site.
    pub origin: String,
    pub kind: PermissionKind,
    pub allow: bool,
}

/// Contents of `permissions.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PermissionsFile {
    #[serde(default)]
    pub rules: Vec<PermissionRule>,
}

impl PermissionsFile {
    /// The stored answer for `origin` + `kind`: the site's own rule, else the `*` rule.
    pub fn decide(&self, origin: &str, kind: PermissionKind) -> Option<bool> {
        let origin = normalize_origin(origin);
        let find = |o: &str| {
            self.rules
                .iter()
                .find(|r| r.kind == kind && r.origin == o)
                .map(|r| r.allow)
        };
        find(&origin).or_else(|| find("*"))
    }

    /// Add/replace (`Some(allow)`) or remove (`None`) the rule for `origin` + `kind`.
    pub fn set(&mut self, origin: &str, kind: PermissionKind, allow: Option<bool>) {
        let origin = normalize_origin(origin);
        self.rules
            .retain(|r| !(r.origin == origin && r.kind == kind));
        if let Some(allow) = allow {
            self.rules.push(PermissionRule {
                origin,
                kind,
                allow,
            });
        }
        self.rules
            .sort_by(|a, b| (&a.origin, a.kind).cmp(&(&b.origin, b.kind)));
    }
}

/// The rules, shared between the CEF permission handler (UI thread) and the commands.
/// Cheap to clone (one `Arc`); every clone sees the same rules.
#[derive(Clone)]
pub struct PermissionStore {
    inner: Arc<Inner>,
}

struct Inner {
    path: PathBuf,
    rules: RwLock<PermissionsFile>,
}

impl PermissionStore {
    /// Load `path` (normally `data/config/permissions.json`); a missing or broken file
    /// means "no rules" (everything denied).
    pub fn load(path: PathBuf) -> Self {
        let rules = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        PermissionStore {
            inner: Arc::new(Inner {
                path,
                rules: RwLock::new(rules),
            }),
        }
    }

    /// Allow only when every kind has an `allow: true` rule; otherwise deny.
    pub fn decide_all(&self, origin: &str, kinds: &[PermissionKind]) -> bool {
        let rules = self.inner.rules.read().unwrap_or_else(|e| e.into_inner());
        !kinds.is_empty()
            && kinds.iter().all(|k| {
                rules
                    .decide(origin, *k)
                    .unwrap_or_else(|| k.allowed_by_default())
            })
    }

    pub fn decide(&self, origin: &str, kind: PermissionKind) -> Option<bool> {
        let rules = self.inner.rules.read().unwrap_or_else(|e| e.into_inner());
        rules.decide(origin, kind)
    }

    pub fn rules(&self) -> Vec<PermissionRule> {
        let rules = self.inner.rules.read().unwrap_or_else(|e| e.into_inner());
        rules.rules.clone()
    }

    /// Change one rule and write the file.
    pub fn set(
        &self,
        origin: &str,
        kind: PermissionKind,
        allow: Option<bool>,
    ) -> Result<(), CmdError> {
        let mut rules = self.inner.rules.write().unwrap_or_else(|e| e.into_inner());
        rules.set(origin, kind, allow);
        let text =
            serde_json::to_string_pretty(&*rules).map_err(|e| CmdError::internal(e.to_string()))?;
        if let Some(dir) = self.inner.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        std::fs::write(&self.inner.path, text).map_err(|e| {
            CmdError::internal(format!("cannot write {}: {e}", self.inner.path.display()))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use PermissionKind::*;

    #[test]
    fn prompt_bits() {
        assert_eq!(kinds_from_prompt_bits(4 | 4096), vec![Camera, Microphone]);
        assert_eq!(
            kinds_from_prompt_bits(1 << 27 | 1 << 26),
            vec![LocalNetwork]
        );
        assert_eq!(kinds_from_prompt_bits(1 << 8), vec![Geolocation]);
        assert_eq!(kinds_from_prompt_bits(1), vec![Other]); // AR session
        assert!(kinds_from_prompt_bits(0).is_empty());
    }

    #[test]
    fn media_bits() {
        assert_eq!(kinds_from_media_bits(3), vec![Camera, Microphone]);
        assert_eq!(kinds_from_media_bits(8), vec![ScreenCapture]);
    }

    #[test]
    fn origins() {
        assert_eq!(normalize_origin("https://A.test/x?y"), "https://a.test");
        assert_eq!(
            normalize_origin("https://a.test:8443"),
            "https://a.test:8443"
        );
        assert_eq!(normalize_origin(" * "), "*");
    }

    #[test]
    fn rules_site_then_wildcard_then_deny() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("permissions.json");
        let store = PermissionStore::load(path.clone());
        assert!(!store.decide_all("https://a.test", &[Camera])); // default: deny
        assert!(store.decide_all("https://a.test", &[MultipleDownloads])); // except this
        store
            .set("https://a.test", MultipleDownloads, Some(false))
            .unwrap();
        assert!(!store.decide_all("https://a.test", &[MultipleDownloads]));
        store
            .set("https://a.test", MultipleDownloads, None)
            .unwrap();
        store.set("*", Geolocation, Some(true)).unwrap();
        store
            .set("https://a.test/page", Camera, Some(true))
            .unwrap();
        assert!(store.decide_all("https://a.test", &[Camera]));
        assert!(!store.decide_all("https://a.test", &[Camera, Microphone]));
        assert!(store.decide_all("https://b.test", &[Geolocation]));
        store
            .set("https://b.test", Geolocation, Some(false))
            .unwrap();
        assert!(!store.decide_all("https://b.test", &[Geolocation]));
        assert!(!store.decide_all("https://b.test", &[]));
        // Persisted, and removable.
        let reloaded = PermissionStore::load(path);
        assert_eq!(reloaded.rules().len(), 3);
        reloaded.set("https://b.test", Geolocation, None).unwrap();
        assert!(reloaded.decide_all("https://b.test", &[Geolocation]));
    }
}
