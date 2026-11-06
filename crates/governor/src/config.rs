//! Limits: a [`Profile`] picked from the machine's RAM, plus user overrides from
//! `data/config/governor.json` (written by `governor.config`).

use std::path::Path;
use std::time::Duration;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// File name inside `data/config/`.
pub const CONFIG_FILE: &str = "governor.json";

const GIB: u64 = 1024 * 1024 * 1024;

/// Hardware class. Chosen automatically from total RAM, overridable in the config.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Profile {
    /// ≤ 8 GB RAM: freeze after 2 min, discard after 10 min, at most 8 live tabs.
    Low,
    /// Around 16 GB: freeze after 5 min, discard after 30 min, at most 20 live tabs.
    Normal,
    /// ≥ 32 GB: freeze after 10 min, discard after 60 min, at most 40 live tabs.
    High,
}

impl Profile {
    /// The profile for a machine with `total` bytes of RAM. `0` (unknown) → `Normal`.
    ///
    /// Machines report a little less than the RAM printed on the box (an "8 GB" laptop shows
    /// ~7.8 GB, a "32 GB" desktop ~31.8 GB), so the borders have some slack.
    pub fn for_total_ram(total: u64) -> Profile {
        if total == 0 {
            Profile::Normal
        } else if total <= 9 * GIB {
            Profile::Low
        } else if total < 30 * GIB {
            Profile::Normal
        } else {
            Profile::High
        }
    }

    /// The default limits of this profile.
    pub fn limits(self) -> Limits {
        let (freeze, discard, max_live, pressure) = match self {
            Profile::Low => (2 * 60, 10 * 60, 8, 20),
            Profile::Normal => (5 * 60, 30 * 60, 20, 15),
            Profile::High => (10 * 60, 60 * 60, 40, 10),
        };
        Limits {
            enabled: true,
            profile: self,
            freeze_after_secs: freeze,
            discard_after_secs: discard,
            max_live_tabs: max_live,
            pressure_free_pct: pressure,
            busy_secs: 60,
            tick_secs: 15,
        }
    }
}

/// The limits in force (profile defaults with the overrides applied).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct Limits {
    /// False = the automatic policy does nothing (manual `tabs.freeze` still works).
    pub enabled: bool,
    pub profile: Profile,
    /// Freeze a tab this long in the background (0 = never).
    pub freeze_after_secs: u64,
    /// Discard a tab this long in the background (0 = never).
    pub discard_after_secs: u64,
    /// Discard the longest-idle tabs when more than this many are not discarded (0 = no limit).
    pub max_live_tabs: usize,
    /// Memory pressure = available RAM below this percentage of total RAM.
    pub pressure_free_pct: u8,
    /// A tab that got a command less than this long ago is "in use" and never touched.
    pub busy_secs: u64,
    /// How often the policy looks at the tabs.
    pub tick_secs: u64,
}

impl Limits {
    pub fn freeze_after(&self) -> Option<Duration> {
        (self.freeze_after_secs > 0).then(|| Duration::from_secs(self.freeze_after_secs))
    }

    pub fn discard_after(&self) -> Option<Duration> {
        (self.discard_after_secs > 0).then(|| Duration::from_secs(self.discard_after_secs))
    }

    pub fn busy(&self) -> Duration {
        Duration::from_secs(self.busy_secs)
    }

    pub fn tick(&self) -> Duration {
        Duration::from_secs(self.tick_secs.max(1))
    }
}

/// User overrides (`data/config/governor.json`). Every field is optional; a missing field
/// keeps the profile's default. `profile` missing = chosen from RAM.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GovernorConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<Profile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub freeze_after_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discard_after_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_live_tabs: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pressure_free_pct: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub busy_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tick_secs: Option<u64>,
}

impl GovernorConfig {
    /// Copy every field that is set in `patch` over ours.
    pub fn merge(&mut self, patch: &GovernorConfig) {
        fn set<T: Clone>(dst: &mut Option<T>, src: &Option<T>) {
            if src.is_some() {
                dst.clone_from(src);
            }
        }
        set(&mut self.enabled, &patch.enabled);
        set(&mut self.profile, &patch.profile);
        set(&mut self.freeze_after_secs, &patch.freeze_after_secs);
        set(&mut self.discard_after_secs, &patch.discard_after_secs);
        set(&mut self.max_live_tabs, &patch.max_live_tabs);
        set(&mut self.pressure_free_pct, &patch.pressure_free_pct);
        set(&mut self.busy_secs, &patch.busy_secs);
        set(&mut self.tick_secs, &patch.tick_secs);
    }

    /// The limits in force: the chosen profile's defaults with our overrides on top.
    /// `auto` is the profile picked from RAM, used unless `profile` is set.
    pub fn limits(&self, auto: Profile) -> Limits {
        let mut limits = self.profile.unwrap_or(auto).limits();
        limits.enabled = self.enabled.unwrap_or(limits.enabled);
        limits.freeze_after_secs = self.freeze_after_secs.unwrap_or(limits.freeze_after_secs);
        limits.discard_after_secs = self.discard_after_secs.unwrap_or(limits.discard_after_secs);
        limits.max_live_tabs = self.max_live_tabs.unwrap_or(limits.max_live_tabs);
        limits.pressure_free_pct = self
            .pressure_free_pct
            .unwrap_or(limits.pressure_free_pct)
            .min(100);
        limits.busy_secs = self.busy_secs.unwrap_or(limits.busy_secs);
        limits.tick_secs = self.tick_secs.unwrap_or(limits.tick_secs).max(1);
        limits
    }

    /// Read the file; a missing or broken file gives no overrides (broken = logged).
    pub fn load(path: &Path) -> GovernorConfig {
        let Ok(text) = std::fs::read_to_string(path) else {
            return GovernorConfig::default();
        };
        serde_json::from_str(&text).unwrap_or_else(|err| {
            tracing::warn!(%err, path = %path.display(), "ignoring broken governor config");
            GovernorConfig::default()
        })
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(path, text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_follows_total_ram() {
        assert_eq!(Profile::for_total_ram(0), Profile::Normal);
        assert_eq!(Profile::for_total_ram(4 * GIB), Profile::Low);
        assert_eq!(Profile::for_total_ram(7 * GIB + GIB / 2), Profile::Low);
        assert_eq!(Profile::for_total_ram(16 * GIB), Profile::Normal);
        assert_eq!(Profile::for_total_ram(31 * GIB), Profile::High);
        assert_eq!(Profile::for_total_ram(64 * GIB), Profile::High);
        let low = Profile::Low.limits();
        assert_eq!((low.freeze_after_secs, low.discard_after_secs), (120, 600));
        assert_eq!(low.max_live_tabs, 8);
    }

    #[test]
    fn overrides_merge_and_apply() {
        let mut cfg = GovernorConfig::default();
        cfg.merge(&GovernorConfig {
            freeze_after_secs: Some(30),
            ..Default::default()
        });
        cfg.merge(&GovernorConfig {
            profile: Some(Profile::Low),
            tick_secs: Some(0),
            ..Default::default()
        });
        assert_eq!(cfg.freeze_after_secs, Some(30)); // kept by the second merge
        let limits = cfg.limits(Profile::High);
        assert_eq!(limits.profile, Profile::Low);
        assert_eq!(limits.freeze_after_secs, 30);
        assert_eq!(limits.discard_after_secs, 600);
        assert_eq!(limits.tick_secs, 1);
        assert_eq!(limits.freeze_after(), Some(Duration::from_secs(30)));
        let off = GovernorConfig {
            discard_after_secs: Some(0),
            ..Default::default()
        };
        assert_eq!(off.limits(Profile::Normal).discard_after(), None);
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config").join(CONFIG_FILE);
        assert_eq!(GovernorConfig::load(&path), GovernorConfig::default());
        let cfg = GovernorConfig {
            enabled: Some(false),
            max_live_tabs: Some(3),
            ..Default::default()
        };
        cfg.save(&path).unwrap();
        assert_eq!(GovernorConfig::load(&path), cfg);
        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(GovernorConfig::load(&path), GovernorConfig::default());
    }
}
