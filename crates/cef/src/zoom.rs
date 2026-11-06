//! Page zoom per tab, remembered per host in `data/config/zoom.json` (`{"example.com": 125}`).
//!
//! * `page.zoom` / Ctrl+= / Ctrl+- / Ctrl+0 (the UI's `zoom.*` commands call `page.zoom`)
//!   → [`change`]: `SetZoomLevel` on the tab and on every other tab of the same host, and
//!   the host's zoom is stored (100 % removes it).
//! * A page commits a new URL → [`apply_stored`]: the host's stored zoom (or 100 %).
//! * Ctrl+wheel zooms inside Chromium without telling us; when Ctrl is released we read the
//!   shown tabs' zoom back ([`check_after_ctrl`]) and store it like any other change.
//!
//! Percent ↔ Chromium zoom level math: `bmcp_core::zoom`. UI thread only.

use std::collections::BTreeMap;
use std::sync::Mutex;

use bmcp_core::zoom::{
    clamp_percent, host_of, is_default, level_from_percent, percent_from_level, step,
};
use bmcp_core::{BackendError, TabId, ZoomChange};
use cef::*;

use crate::bridge::{emit_updated, with_ui};

/// host → percent, loaded on first use.
static STORE: Mutex<Option<BTreeMap<String, f64>>> = Mutex::new(None);

fn path() -> std::path::PathBuf {
    crate::profiles::launch()
        .data_root
        .join("config")
        .join("zoom.json")
}

fn with_store<T>(f: impl FnOnce(&mut BTreeMap<String, f64>) -> T) -> T {
    let mut store = STORE.lock().unwrap_or_else(|e| e.into_inner());
    let map = store.get_or_insert_with(|| {
        std::fs::read_to_string(path())
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    });
    f(map)
}

/// The stored zoom of `host` (100 when none).
pub(crate) fn stored(host: &str) -> f64 {
    with_store(|m| m.get(host).copied()).unwrap_or(100.0)
}

/// Remember `percent` for `host` (100 % forgets it) and write the file.
fn remember(host: &str, percent: f64) {
    let text = with_store(|m| {
        update_map(m, host, percent);
        serde_json::to_string_pretty(m).unwrap_or_default()
    });
    let path = path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(err) = std::fs::write(&path, text) {
        tracing::warn!(path = %path.display(), %err, "zoom.json not written");
    }
}

/// Set a browser's zoom and the tab entry's copy; `tab.updated` when it changed.
fn set(id: TabId, browser: &Browser, percent: f64) {
    if let Some(host) = browser.host() {
        host.set_zoom_level(level_from_percent(percent));
    }
    let changed = with_ui(|ui| {
        let t = ui.tabs.get_mut(&id)?;
        let changed = (t.zoom - percent).abs() >= 0.5;
        t.zoom = percent;
        Some(changed)
    });
    if changed == Some(true) {
        emit_updated(id);
    }
}

/// `page.zoom`: change tab `id`'s zoom; every open tab of the same host follows, and the
/// host remembers it. Returns the new percentage.
pub(crate) fn change(id: TabId, how: ZoomChange) -> Result<f64, BackendError> {
    let (browser, url, current) = with_ui(|ui| {
        ui.tabs
            .get(&id)
            .map(|t| (t.browser.clone(), t.url.clone(), t.zoom))
    })
    .ok_or(BackendError::TabNotFound(id))?;
    let percent = match how {
        ZoomChange::Percent(p) => clamp_percent(p),
        ZoomChange::Steps(n) => step(current, n),
    };
    set(id, &browser, percent);
    if let Some(host) = host_of(&url) {
        remember(&host, percent);
        let same_host: Vec<(TabId, Browser)> = with_ui(|ui| {
            ui.tabs
                .iter()
                .filter(|(other, t)| **other != id && host_of(&t.url).as_deref() == Some(&host))
                .map(|(other, t)| (*other, t.browser.clone()))
                .collect()
        });
        for (other, browser) in same_host {
            set(other, &browser, percent);
        }
    }
    Ok(percent)
}

/// A tab committed a new main-frame URL: give it its host's stored zoom.
pub(crate) fn apply_stored(id: TabId) {
    let Some((browser, url, current)) = with_ui(|ui| {
        ui.tabs
            .get(&id)
            .map(|t| (t.browser.clone(), t.url.clone(), t.zoom))
    }) else {
        return;
    };
    // Our own pages (new tab page, library) always show at 100 %.
    let want = host_of(&url)
        .filter(|_| !url.starts_with("bmcp:"))
        .map_or(100.0, |h| stored(&h));
    // Chromium may keep a per-host zoom of its own: read the real value as well.
    let actual = browser
        .host()
        .map_or(current, |h| percent_from_level(h.zoom_level()));
    if (want - actual).abs() >= 0.5 || (want - current).abs() >= 0.5 {
        set(id, &browser, want);
    }
}

/// Ctrl was released in window `key`: a Ctrl+wheel zoom may have happened in a shown tab.
/// Read the shown tabs' zoom back and remember a change.
pub(crate) fn check_after_ctrl(key: i32) {
    let shown: Vec<(TabId, Browser, String, f64)> = with_ui(|ui| {
        let Some(w) = ui.windows.get(&key) else {
            return Vec::new();
        };
        let mut ids: Vec<TabId> = w.strip.active().into_iter().collect();
        if let Some(split) = w.split {
            ids = vec![split.left, split.right];
        }
        ids.iter()
            .filter_map(|id| {
                let t = ui.tabs.get(id)?;
                Some((*id, t.browser.clone(), t.url.clone(), t.zoom))
            })
            .collect()
    });
    for (id, browser, url, known) in shown {
        let Some(level) = browser.host().map(|h| h.zoom_level()) else {
            continue;
        };
        let actual = percent_from_level(level);
        if (actual - known).abs() < 0.5 {
            continue;
        }
        tracing::debug!(%id, known, actual, "zoom changed in the page (Ctrl+wheel)");
        with_ui(|ui| {
            if let Some(t) = ui.tabs.get_mut(&id) {
                t.zoom = actual;
            }
        });
        emit_updated(id);
        if let Some(host) = host_of(&url).filter(|_| !url.starts_with("bmcp:")) {
            remember(&host, actual);
        }
    }
}

/// The per-host map after a zoom change (pure): 100 % is the default and is not stored.
pub(crate) fn update_map(map: &mut BTreeMap<String, f64>, host: &str, percent: f64) {
    if is_default(percent) {
        map.remove(host);
    } else {
        map.insert(host.to_string(), percent);
    }
}

/// The tab entry's zoom as `TabInfo::zoom` (absent at 100 %).
pub(crate) fn badge(percent: f64) -> Option<u16> {
    (!is_default(percent)).then(|| percent.round().clamp(0.0, f64::from(u16::MAX)) as u16)
}

#[cfg(test)]
mod tests {
    use super::{badge, update_map};

    #[test]
    fn map_stores_only_non_default_zoom() {
        let mut map = std::collections::BTreeMap::new();
        update_map(&mut map, "a.test", 125.0);
        update_map(&mut map, "b.test", 90.0);
        assert_eq!(map.get("a.test"), Some(&125.0));
        update_map(&mut map, "a.test", 100.0);
        assert!(!map.contains_key("a.test"));
        assert_eq!(serde_json::to_string(&map).unwrap(), r#"{"b.test":90.0}"#);
    }

    #[test]
    fn badge_only_when_not_100() {
        assert_eq!(badge(100.0), None);
        assert_eq!(badge(100.3), None);
        assert_eq!(badge(125.0), Some(125));
        assert_eq!(badge(33.0), Some(33));
    }
}
