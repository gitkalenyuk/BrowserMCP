//! Page zoom math (pure): Chrome's zoom presets and Chromium's zoom levels.
//!
//! Chromium stores zoom as a *level*: `percent = 100 * 1.2^level` (level 0 = 100 %).
//! People and the API use percent; Ctrl+= / Ctrl+- step through Chrome's presets.

/// Chrome's zoom presets, in percent.
pub const PRESETS: [f64; 17] = [
    25.0, 33.0, 50.0, 67.0, 75.0, 80.0, 90.0, 100.0, 110.0, 125.0, 150.0, 175.0, 200.0, 250.0,
    300.0, 400.0, 500.0,
];

/// Smallest and largest zoom we accept (the first and last preset).
pub const MIN_PERCENT: f64 = 25.0;
pub const MAX_PERCENT: f64 = 500.0;

/// Chromium zoom level for a percentage (clamped to 25–500 %).
pub fn level_from_percent(percent: f64) -> f64 {
    let percent = clamp_percent(percent);
    (percent / 100.0).ln() / 1.2f64.ln()
}

/// Percentage for a Chromium zoom level, rounded to a whole percent.
pub fn percent_from_level(level: f64) -> f64 {
    (100.0 * 1.2f64.powf(level)).round()
}

/// `percent` limited to 25–500 (NaN → 100).
pub fn clamp_percent(percent: f64) -> f64 {
    if percent.is_nan() {
        return 100.0;
    }
    percent.clamp(MIN_PERCENT, MAX_PERCENT)
}

/// The zoom after `steps` preset steps from `current` (0 steps = reset to 100 %). A zoom
/// between two presets moves to the next preset in that direction first.
pub fn step(current: f64, steps: i32) -> f64 {
    if steps == 0 {
        return 100.0;
    }
    let mut value = clamp_percent(current);
    for _ in 0..steps.unsigned_abs() {
        // Tolerance: 33 % stored as level and read back can be 33.0001.
        value = if steps > 0 {
            PRESETS
                .iter()
                .copied()
                .find(|p| *p > value + 0.5)
                .unwrap_or(MAX_PERCENT)
        } else {
            PRESETS
                .iter()
                .rev()
                .copied()
                .find(|p| *p < value - 0.5)
                .unwrap_or(MIN_PERCENT)
        };
    }
    value
}

/// Is this "normal size" (shown without a zoom badge)?
pub fn is_default(percent: f64) -> bool {
    (percent - 100.0).abs() < 0.5
}

/// The host part of a URL (`https://user@a.test:8080/x` → `a.test`), lower-case; zoom is
/// remembered per host. `None` for URLs without one (`about:blank`, `data:`, `file:`).
pub fn host_of(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host_port = authority.rsplit('@').next()?;
    let host = if let Some(v6) = host_port.strip_prefix('[') {
        v6.split(']').next()?
    } else {
        host_port.split(':').next()?
    };
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_and_percent() {
        assert!((level_from_percent(100.0)).abs() < 1e-9);
        assert!((level_from_percent(120.0) - 1.0).abs() < 1e-9);
        assert_eq!(percent_from_level(level_from_percent(125.0)), 125.0);
        assert_eq!(percent_from_level(level_from_percent(33.0)), 33.0);
        // Clamped.
        assert_eq!(percent_from_level(level_from_percent(1000.0)), 500.0);
        assert_eq!(clamp_percent(f64::NAN), 100.0);
    }

    #[test]
    fn steps_follow_the_presets() {
        assert_eq!(step(100.0, 1), 110.0);
        assert_eq!(step(100.0, -1), 90.0);
        assert_eq!(step(100.0, 2), 125.0);
        assert_eq!(step(110.0, -1), 100.0);
        assert_eq!(step(500.0, 1), 500.0);
        assert_eq!(step(25.0, -1), 25.0);
        // Between presets: to the next one in that direction.
        assert_eq!(step(105.0, 1), 110.0);
        assert_eq!(step(105.0, -1), 100.0);
        // Rounding noise of a stored level does not skip a preset.
        assert_eq!(step(33.0004, 1), 50.0);
        assert_eq!(step(175.0, 0), 100.0);
        assert!(is_default(100.2));
        assert!(!is_default(110.0));
    }

    #[test]
    fn hosts() {
        assert_eq!(
            host_of("https://Example.com/a?b").as_deref(),
            Some("example.com")
        );
        assert_eq!(
            host_of("http://u:p@a.test:8080/").as_deref(),
            Some("a.test")
        );
        assert_eq!(host_of("http://[::1]:80/").as_deref(), Some("::1"));
        assert_eq!(host_of("bmcp://ui/newtab.html").as_deref(), Some("ui"));
        assert_eq!(host_of("about:blank"), None);
        assert_eq!(host_of("file:///C:/x.html"), None);
    }
}
