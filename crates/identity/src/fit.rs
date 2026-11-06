//! Fit an identity to where its proxy's exit IP is (`identity.fit_to_proxy`): a German IP
//! with an "America/New_York" clock and English-only languages is a classic tell.
//!
//! Changes only what belongs to the place — timezone, locale, languages (Accept-Language) and
//! the Geolocation API position — and keeps the device (user agent, platform, screen, GPU).
//! Pure: the geo lookup happens in the command; tests pass a [`GeoInfo`] directly.

use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{Value, json};

use crate::country;
use crate::identity::{Geolocation, Identity, accept_language};
use crate::proxy::GeoInfo;
use crate::rng::Rng;

/// One changed field.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct Change {
    /// Identity field, e.g. "timezone".
    pub field: String,
    pub from: Value,
    pub to: Value,
}

/// Largest random offset of the position from the city centre, in degrees (~2 km), so
/// identities in one city do not all sit on the same point.
const JITTER_DEGREES: f64 = 0.02;

/// Adapt `identity` to `geo` in place and list what changed (empty = it already fit).
///
/// * timezone: the lookup's zone, else the country's main zone from the offline table;
/// * locale: the region's or country's usual language (table), else the lookup's suggestion;
/// * accept_language: derived from the locale (`de-CH,de,en-US,en`);
/// * geolocation: the lookup's coordinates (else the main city's) plus a small jitter that is
///   deterministic for (identity id, IP), so fitting again to the same IP changes nothing.
pub fn fit_to_geo(identity: &mut Identity, geo: &GeoInfo) -> Vec<Change> {
    let mut changes = Vec::new();
    let mut record = |field: &str, from: Value, to: Value| {
        if from != to {
            changes.push(Change {
                field: field.to_string(),
                from,
                to,
            });
        }
    };
    let table = country::country(&geo.country_code);

    // Timezone.
    let timezone = if geo.timezone.contains('/') {
        Some(geo.timezone.clone())
    } else {
        table.map(|c| c.timezone.to_string())
    };
    if let Some(tz) = timezone {
        record("timezone", json!(identity.timezone), json!(tz));
        identity.timezone = tz;
    }

    // Locale and languages.
    let locale = country::locale_for_place(&geo.country_code, &geo.region)
        .map(str::to_string)
        .or_else(|| geo.locale.clone());
    if let Some(locale) = locale {
        let languages = accept_language(&locale);
        record("locale", json!(identity.locale), json!(locale));
        record(
            "accept_language",
            json!(identity.accept_language),
            json!(languages),
        );
        identity.locale = locale;
        identity.accept_language = languages;
    }

    // Position.
    let known = geo.latitude != 0.0 || geo.longitude != 0.0;
    let centre = if known {
        Some((geo.latitude, geo.longitude))
    } else {
        table.map(|c| (c.latitude, c.longitude))
    };
    if let Some((lat, lon)) = centre {
        let mut rng = Rng::new(seed_for(&identity.id, &geo.ip));
        let mut jitter = || (rng.below(2001) as f64 / 1000.0 - 1.0) * JITTER_DEGREES;
        let (dlat, dlon) = (jitter(), jitter());
        let accuracy = identity.geolocation.map_or(50.0, |g| g.accuracy);
        let new = Geolocation {
            latitude: round4((lat + dlat).clamp(-90.0, 90.0)),
            longitude: round4((lon + dlon).clamp(-180.0, 180.0)),
            accuracy,
        };
        record("geolocation", json!(identity.geolocation), json!(Some(new)));
        identity.geolocation = Some(new);
    }
    changes
}

fn round4(v: f64) -> f64 {
    (v * 1e4).round() / 1e4
}

/// FNV-1a of "id|ip": a stable seed for the jitter.
fn seed_for(id: &str, ip: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in id.bytes().chain(*b"|").chain(ip.bytes()) {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::{Overrides, generate, problems};

    fn geo(cc: &str, region: &str, tz: &str, lat: f64, lon: f64) -> GeoInfo {
        GeoInfo {
            ip: "203.0.113.9".into(),
            country: String::new(),
            country_code: cc.into(),
            region: region.into(),
            city: String::new(),
            timezone: tz.into(),
            latitude: lat,
            longitude: lon,
            isp: String::new(),
            locale: None,
            accept_language: None,
        }
    }

    #[test]
    fn fits_timezone_language_and_position_but_keeps_the_device() {
        let mut id = generate(None, 7, &Overrides::default());
        let before = id.clone();
        let changes = fit_to_geo(
            &mut id,
            &geo("CH", "Geneva", "Europe/Zurich", 46.2044, 6.1432),
        );
        assert_eq!(id.timezone, "Europe/Zurich");
        assert_eq!(id.locale, "fr-CH");
        assert_eq!(id.accept_language, "fr-CH,fr,en-US,en");
        let g = id.geolocation.unwrap();
        assert!((g.latitude - 46.2044).abs() <= JITTER_DEGREES + 1e-9);
        assert!((g.longitude - 6.1432).abs() <= JITTER_DEGREES + 1e-9);
        let fields: Vec<&str> = changes.iter().map(|c| c.field.as_str()).collect();
        assert_eq!(
            fields,
            ["timezone", "locale", "accept_language", "geolocation"]
        );
        assert_eq!(changes[0].from, json!(before.timezone));
        // The device stays.
        assert_eq!(id.user_agent, before.user_agent);
        assert_eq!(id.platform, before.platform);
        assert_eq!(id.ua_metadata, before.ua_metadata);
        assert_eq!(id.screen, before.screen);
        assert!(problems(&id).is_empty(), "{:?}", problems(&id));
        // Fitting again to the same IP changes nothing.
        assert!(
            fit_to_geo(
                &mut id,
                &geo("CH", "Geneva", "Europe/Zurich", 46.2044, 6.1432)
            )
            .is_empty()
        );
    }

    #[test]
    fn falls_back_to_the_offline_table() {
        let mut id = generate(None, 3, &Overrides::default());
        // No timezone or coordinates from the lookup: the table fills them in.
        fit_to_geo(&mut id, &geo("JP", "", "", 0.0, 0.0));
        assert_eq!(id.timezone, "Asia/Tokyo");
        assert_eq!(id.locale, "ja-JP");
        let g = id.geolocation.unwrap();
        assert!((g.latitude - 35.6762).abs() < 0.05, "{g:?}");
        // Unknown country without data: nothing to change except what the lookup gave.
        let mut other = generate(None, 3, &Overrides::default());
        let before = other.clone();
        assert!(fit_to_geo(&mut other, &geo("ZZ", "", "", 0.0, 0.0)).is_empty());
        assert_eq!(other, before);
        // An unknown country with the lookup's own suggestion.
        let mut g = geo("ZZ", "", "Etc/UTC", 0.0, 0.0);
        g.locale = Some("en-US".into());
        let changes = fit_to_geo(&mut other, &g);
        assert_eq!(other.timezone, "Etc/UTC");
        assert!(changes.iter().any(|c| c.field == "timezone"));
    }

    #[test]
    fn jitter_differs_per_identity_and_ip() {
        let a = seed_for("win-1", "1.2.3.4");
        assert_eq!(a, seed_for("win-1", "1.2.3.4"));
        assert_ne!(a, seed_for("win-2", "1.2.3.4"));
        assert_ne!(a, seed_for("win-1", "1.2.3.5"));
    }
}
