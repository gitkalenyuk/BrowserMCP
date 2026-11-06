//! Offline country table: default timezone, the languages people there use (Chrome locale
//! tags, most common first) and the main city's coordinates.
//!
//! Used to fit an identity to a proxy's exit IP (`identity.fit_to_proxy`) when the geo lookup
//! leaves something out, and to pick the locale for a country. No network: tests and offline
//! use work the same. For countries with several time zones the default is the zone where
//! most people live; the geo lookup normally returns the exact zone anyway.

/// What we know about one country.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Country {
    /// ISO 3166-1 alpha-2 code, upper case ("DE").
    pub code: &'static str,
    /// IANA zone where most people live.
    pub timezone: &'static str,
    /// Locales, most common first ("de-CH", "fr-CH", "it-CH").
    pub languages: &'static [&'static str],
    /// Main city (latitude, longitude).
    pub latitude: f64,
    pub longitude: f64,
}

/// Shorthand for the table below.
const fn c(
    code: &'static str,
    timezone: &'static str,
    languages: &'static [&'static str],
    latitude: f64,
    longitude: f64,
) -> Country {
    Country {
        code,
        timezone,
        languages,
        latitude,
        longitude,
    }
}

/// The table (alphabetical by code).
pub const COUNTRIES: &[Country] = &[
    c("AE", "Asia/Dubai", &["ar-AE", "en-AE"], 25.2048, 55.2708),
    c("AM", "Asia/Yerevan", &["hy-AM", "ru-AM"], 40.1792, 44.4991),
    c(
        "AR",
        "America/Argentina/Buenos_Aires",
        &["es-AR"],
        -34.6037,
        -58.3816,
    ),
    c("AT", "Europe/Vienna", &["de-AT"], 48.2082, 16.3738),
    c("AU", "Australia/Sydney", &["en-AU"], -33.8688, 151.2093),
    c("AZ", "Asia/Baku", &["az-AZ", "ru-AZ"], 40.4093, 49.8671),
    c(
        "BA",
        "Europe/Sarajevo",
        &["bs-BA", "hr-BA", "sr-BA"],
        43.8563,
        18.4131,
    ),
    c("BD", "Asia/Dhaka", &["bn-BD", "en-BD"], 23.8103, 90.4125),
    c(
        "BE",
        "Europe/Brussels",
        &["nl-BE", "fr-BE", "de-BE"],
        50.8503,
        4.3517,
    ),
    c("BG", "Europe/Sofia", &["bg-BG"], 42.6977, 23.3219),
    c("BR", "America/Sao_Paulo", &["pt-BR"], -23.5505, -46.6333),
    c("BY", "Europe/Minsk", &["ru-BY", "be-BY"], 53.9006, 27.5590),
    c(
        "CA",
        "America/Toronto",
        &["en-CA", "fr-CA"],
        43.6532,
        -79.3832,
    ),
    c(
        "CH",
        "Europe/Zurich",
        &["de-CH", "fr-CH", "it-CH"],
        47.3769,
        8.5417,
    ),
    c("CL", "America/Santiago", &["es-CL"], -33.4489, -70.6693),
    c("CN", "Asia/Shanghai", &["zh-CN"], 31.2304, 121.4737),
    c("CO", "America/Bogota", &["es-CO"], 4.7110, -74.0721),
    c("CY", "Asia/Nicosia", &["el-CY", "en-CY"], 35.1856, 33.3823),
    c("CZ", "Europe/Prague", &["cs-CZ"], 50.0755, 14.4378),
    c("DE", "Europe/Berlin", &["de-DE"], 52.5200, 13.4050),
    c("DK", "Europe/Copenhagen", &["da-DK"], 55.6761, 12.5683),
    c(
        "EE",
        "Europe/Tallinn",
        &["et-EE", "ru-EE"],
        59.4370,
        24.7536,
    ),
    c("EG", "Africa/Cairo", &["ar-EG"], 30.0444, 31.2357),
    c("ES", "Europe/Madrid", &["es-ES", "ca-ES"], 40.4168, -3.7038),
    c(
        "FI",
        "Europe/Helsinki",
        &["fi-FI", "sv-FI"],
        60.1699,
        24.9384,
    ),
    c("FR", "Europe/Paris", &["fr-FR"], 48.8566, 2.3522),
    c("GB", "Europe/London", &["en-GB"], 51.5074, -0.1278),
    c("GE", "Asia/Tbilisi", &["ka-GE"], 41.7151, 44.8271),
    c("GR", "Europe/Athens", &["el-GR"], 37.9838, 23.7275),
    c(
        "HK",
        "Asia/Hong_Kong",
        &["zh-HK", "en-HK"],
        22.3193,
        114.1694,
    ),
    c("HR", "Europe/Zagreb", &["hr-HR"], 45.8150, 15.9819),
    c("HU", "Europe/Budapest", &["hu-HU"], 47.4979, 19.0402),
    c("ID", "Asia/Jakarta", &["id-ID"], -6.2088, 106.8456),
    c("IE", "Europe/Dublin", &["en-IE"], 53.3498, -6.2603),
    c("IL", "Asia/Jerusalem", &["he-IL"], 32.0853, 34.7818),
    c("IN", "Asia/Kolkata", &["en-IN", "hi-IN"], 19.0760, 72.8777),
    c("IS", "Atlantic/Reykjavik", &["is-IS"], 64.1466, -21.9426),
    c("IT", "Europe/Rome", &["it-IT"], 41.9028, 12.4964),
    c("JP", "Asia/Tokyo", &["ja-JP"], 35.6762, 139.6503),
    c(
        "KE",
        "Africa/Nairobi",
        &["en-KE", "sw-KE"],
        -1.2921,
        36.8219,
    ),
    c("KR", "Asia/Seoul", &["ko-KR"], 37.5665, 126.9780),
    c("KZ", "Asia/Almaty", &["kk-KZ", "ru-KZ"], 43.2220, 76.8512),
    c("LT", "Europe/Vilnius", &["lt-LT"], 54.6872, 25.2797),
    c(
        "LU",
        "Europe/Luxembourg",
        &["fr-LU", "de-LU", "lb-LU"],
        49.6116,
        6.1319,
    ),
    c("LV", "Europe/Riga", &["lv-LV", "ru-LV"], 56.9496, 24.1052),
    c(
        "MA",
        "Africa/Casablanca",
        &["ar-MA", "fr-MA"],
        33.5731,
        -7.5898,
    ),
    c(
        "MD",
        "Europe/Chisinau",
        &["ro-MD", "ru-MD"],
        47.0105,
        28.8638,
    ),
    c("MT", "Europe/Malta", &["en-MT", "mt-MT"], 35.8989, 14.5146),
    c("MX", "America/Mexico_City", &["es-MX"], 19.4326, -99.1332),
    c(
        "MY",
        "Asia/Kuala_Lumpur",
        &["ms-MY", "en-MY"],
        3.1390,
        101.6869,
    ),
    c("NG", "Africa/Lagos", &["en-NG"], 6.5244, 3.3792),
    c("NL", "Europe/Amsterdam", &["nl-NL"], 52.3676, 4.9041),
    c("NO", "Europe/Oslo", &["nb-NO"], 59.9139, 10.7522),
    c("NZ", "Pacific/Auckland", &["en-NZ"], -36.8485, 174.7633),
    c("PE", "America/Lima", &["es-PE"], -12.0464, -77.0428),
    c("PH", "Asia/Manila", &["en-PH", "fil-PH"], 14.5995, 120.9842),
    c("PK", "Asia/Karachi", &["en-PK", "ur-PK"], 24.8607, 67.0011),
    c("PL", "Europe/Warsaw", &["pl-PL"], 52.2297, 21.0122),
    c("PT", "Europe/Lisbon", &["pt-PT"], 38.7223, -9.1393),
    c("RO", "Europe/Bucharest", &["ro-RO"], 44.4268, 26.1025),
    c("RS", "Europe/Belgrade", &["sr-RS"], 44.7866, 20.4489),
    c("RU", "Europe/Moscow", &["ru-RU"], 55.7558, 37.6173),
    c("SA", "Asia/Riyadh", &["ar-SA"], 24.7136, 46.6753),
    c("SE", "Europe/Stockholm", &["sv-SE"], 59.3293, 18.0686),
    c(
        "SG",
        "Asia/Singapore",
        &["en-SG", "zh-SG"],
        1.3521,
        103.8198,
    ),
    c("SI", "Europe/Ljubljana", &["sl-SI"], 46.0569, 14.5058),
    c("SK", "Europe/Bratislava", &["sk-SK"], 48.1486, 17.1077),
    c("TH", "Asia/Bangkok", &["th-TH"], 13.7563, 100.5018),
    c("TR", "Europe/Istanbul", &["tr-TR"], 41.0082, 28.9784),
    c("TW", "Asia/Taipei", &["zh-TW"], 25.0330, 121.5654),
    c("UA", "Europe/Kyiv", &["uk-UA"], 50.4501, 30.5234),
    c(
        "US",
        "America/New_York",
        &["en-US", "es-US"],
        40.7128,
        -74.0060,
    ),
    c("VE", "America/Caracas", &["es-VE"], 10.4806, -66.9036),
    c("VN", "Asia/Ho_Chi_Minh", &["vi-VN"], 10.8231, 106.6297),
    c(
        "ZA",
        "Africa/Johannesburg",
        &["en-ZA", "af-ZA"],
        -26.2041,
        28.0473,
    ),
];

/// The entry for a country code (any case). `None` = not in the table.
pub fn country(code: &str) -> Option<&'static Country> {
    let code = code.trim().to_ascii_uppercase();
    COUNTRIES.iter().find(|c| c.code == code)
}

/// Regions of multilingual countries where another language is the usual one (region names as
/// ip-api.com writes them).
const REGIONAL_LANGUAGES: &[(&str, &str, &str)] = &[
    ("CA", "Quebec", "fr-CA"),
    ("BE", "Wallonia", "fr-BE"),
    ("BE", "Brussels Capital", "fr-BE"),
    ("CH", "Geneva", "fr-CH"),
    ("CH", "Vaud", "fr-CH"),
    ("CH", "Neuchatel", "fr-CH"),
    ("CH", "Neuchâtel", "fr-CH"),
    ("CH", "Jura", "fr-CH"),
    ("CH", "Ticino", "it-CH"),
];

/// The locale people at this place usually have: the region's language for the few
/// multilingual regions we know, else the country's first language.
pub fn locale_for_place(country_code: &str, region: &str) -> Option<&'static str> {
    let cc = country_code.trim().to_ascii_uppercase();
    let regional = REGIONAL_LANGUAGES
        .iter()
        .find(|(c, r, _)| *c == cc && r.eq_ignore_ascii_case(region.trim()))
        .map(|(_, _, locale)| *locale);
    regional.or_else(|| country(&cc).map(|c| c.languages[0]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_is_well_formed() {
        let mut codes: Vec<&str> = COUNTRIES.iter().map(|c| c.code).collect();
        let sorted = {
            let mut s = codes.clone();
            s.sort();
            s
        };
        assert_eq!(codes, sorted, "keep the table alphabetical");
        codes.dedup();
        assert_eq!(codes.len(), COUNTRIES.len(), "duplicate code");
        for c in COUNTRIES {
            assert_eq!(c.code.len(), 2);
            assert!(c.timezone.contains('/'), "{}", c.code);
            assert!(!c.languages.is_empty(), "{}", c.code);
            for l in c.languages {
                // Every locale names this country, e.g. "fr-CH" for CH.
                assert!(l.ends_with(&format!("-{}", c.code)), "{} {l}", c.code);
            }
            assert!(c.latitude.abs() <= 90.0 && c.longitude.abs() <= 180.0);
        }
    }

    #[test]
    fn regions_pick_their_language() {
        assert_eq!(locale_for_place("ca", "Quebec"), Some("fr-CA"));
        assert_eq!(locale_for_place("CA", "Ontario"), Some("en-CA"));
        assert_eq!(locale_for_place("CH", "ticino"), Some("it-CH"));
        assert_eq!(locale_for_place("CH", "Zurich"), Some("de-CH"));
        assert_eq!(locale_for_place("XX", ""), None);
        assert_eq!(country("de").unwrap().timezone, "Europe/Berlin");
    }
}
