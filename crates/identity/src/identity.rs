//! Fingerprint identities: one consistent bundle of everything a page can read about the
//! "device" (user agent, UA client hints, `navigator.platform`, CPU/memory, screen, timezone,
//! language, location, touch, WebGL GPU strings).
//!
//! Consistency matters more than rarity: a Windows user agent with `navigator.platform =
//! "MacIntel"` or a Direct3D GPU string on a "Mac" is an instant tell. So identities are
//! built from [`PRESETS`] (real device classes) and a seeded generator ([`generate`]) that only
//! mixes values which belong together. [`problems`] lists inconsistencies; tests run it over
//! every preset and many generated identities.
//!
//! The engine is Chromium, so the claimed browser is always Chrome with the engine's major
//! version (feature detection would reveal any other version). iPhone/iPad presets claim
//! Safari and are therefore only [`Kind::DeviceEmulation`] (responsive testing), not stealth.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::rng::Rng;

/// Chromium major version of the bundled CEF (156.x). `stealth.fingerprint_set` asks the
/// running engine (`Browser.getVersion`) for the full version and uses it when the majors match.
pub const ENGINE_CHROME_MAJOR: u32 = 156;

/// Values `navigator.deviceMemory` can have (GiB, rounded down to a power of two). Chromium 156
/// reports up to 32 (seen live on a 32 GB machine); older versions capped at 8.
pub const DEVICE_MEMORY_VALUES: [f64; 8] = [0.25, 0.5, 1.0, 2.0, 4.0, 8.0, 16.0, 32.0];

/// Operating system family an identity claims.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Os {
    Windows,
    Macos,
    Android,
    Ios,
}

impl Os {
    pub fn as_str(self) -> &'static str {
        match self {
            Os::Windows => "windows",
            Os::Macos => "macos",
            Os::Android => "android",
            Os::Ios => "ios",
        }
    }
}

/// What an identity is good for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// Consistent with the Chromium engine: meant to look like a normal Chrome user.
    Stealth,
    /// Claims a different engine (Safari on iPhone/iPad): fine for responsive layout testing,
    /// but fingerprinting scripts will see that the engine is Chromium.
    DeviceEmulation,
}

/// One entry of the UA client hints brand lists (`Sec-CH-UA`, `navigator.userAgentData`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Brand {
    pub brand: String,
    pub version: String,
}

/// User-Agent Client Hints (CDP `Emulation.UserAgentMetadata`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UaMetadata {
    /// Low-entropy brands with major versions, e.g. Chromium/156, Google Chrome/156, a GREASE.
    pub brands: Vec<Brand>,
    /// The same brands with full versions (high entropy).
    pub full_version_list: Vec<Brand>,
    /// "Windows", "macOS", "Android".
    pub platform: String,
    /// e.g. "19.0.0" (Windows 11 24H2), "14.5.0" (macOS), "14.0.0" (Android 14).
    pub platform_version: String,
    /// "x86", "arm" or "" (mobile).
    pub architecture: String,
    /// Device model on Android ("Pixel 8"), empty on desktop.
    pub model: String,
    pub mobile: bool,
    /// "64" on desktop, "" on mobile.
    pub bitness: String,
    pub wow64: bool,
}

/// Screen size in CSS pixels and the device pixel ratio.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Screen {
    pub width: u32,
    pub height: u32,
    pub dpr: f64,
}

/// Page viewport (CSS pixels). Desktop identities usually leave it to the real window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Viewport {
    pub width: u32,
    pub height: u32,
}

/// A fake position for the Geolocation API.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Geolocation {
    pub latitude: f64,
    pub longitude: f64,
    /// Metres.
    pub accuracy: f64,
}

/// A complete, consistent fingerprint. Stored in `data/config/identities.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Identity {
    /// Name to refer to it (`identity_id`), e.g. "win-7f3a".
    pub id: String,
    /// The preset it was built from, e.g. "windows-1080p".
    pub preset: String,
    pub kind: Kind,
    pub os: Os,
    /// `navigator.userAgent` and the `User-Agent` header.
    pub user_agent: String,
    /// Client hints; `None` for Safari identities (Safari has none).
    pub ua_metadata: Option<UaMetadata>,
    /// `navigator.platform`, e.g. "Win32", "MacIntel", "Linux armv81".
    pub platform: String,
    pub hardware_concurrency: u32,
    /// `navigator.deviceMemory` in GiB: a power of two from 0.25 to 32 (Chromium 156 reports up
    /// to 32; older versions capped at 8).
    pub device_memory: f64,
    pub screen: Screen,
    /// Forced viewport (mobile). `None` = keep the real window size.
    pub viewport: Option<Viewport>,
    /// IANA zone, e.g. "Europe/Berlin".
    pub timezone: String,
    /// ICU locale for Intl, e.g. "de-DE"; also `navigator.language`.
    pub locale: String,
    /// `navigator.languages` and (with q-values added by Chromium) `Accept-Language`, e.g.
    /// "de-DE,de,en-US,en".
    pub accept_language: String,
    pub geolocation: Option<Geolocation>,
    /// Touch screen (mobile): `maxTouchPoints` > 0 and touch events.
    pub touch: bool,
    pub max_touch_points: u32,
    /// Mobile viewport behaviour (meta viewport, overlay scrollbars).
    pub mobile: bool,
    /// `UNMASKED_VENDOR_WEBGL`, e.g. "Google Inc. (NVIDIA)".
    pub webgl_vendor: String,
    /// `UNMASKED_RENDERER_WEBGL`, e.g. "ANGLE (NVIDIA, NVIDIA GeForce RTX 3060 ...)".
    pub webgl_renderer: String,
}

// ---------------------------------------------------------------- presets

/// A device class the generator builds identities from.
#[derive(Debug, Clone, Copy)]
pub struct Preset {
    /// Key, e.g. "windows-1080p".
    pub name: &'static str,
    /// Human description.
    pub title: &'static str,
    /// Name accepted by `device.emulate` ("iPhone 15"); `None` for desktop presets.
    pub device: Option<&'static str>,
    pub os: Os,
    pub kind: Kind,
    pub screen: Screen,
    pub viewport: Option<Viewport>,
    pub cpus: &'static [u32],
    pub memory: &'static [f64],
    /// (vendor, renderer) pairs that exist on this platform.
    pub gpus: &'static [(&'static str, &'static str)],
    /// OS versions: UA-CH `platformVersion` (Windows/macOS/Android) or iOS "17_5".
    pub os_versions: &'static [&'static str],
    /// Android model / Apple device; empty for PCs.
    pub model: &'static str,
    /// `navigator.platform`.
    pub platform: &'static str,
    pub max_touch_points: u32,
    /// macOS CPU architecture: "arm" (Apple Silicon) or "x86".
    pub arch: &'static str,
}

const WIN_GPUS: &[(&str, &str)] = &[
    (
        "Google Inc. (NVIDIA)",
        "ANGLE (NVIDIA, NVIDIA GeForce RTX 3060 (0x00002504) Direct3D11 vs_5_0 ps_5_0, D3D11)",
    ),
    (
        "Google Inc. (NVIDIA)",
        "ANGLE (NVIDIA, NVIDIA GeForce GTX 1650 (0x00001F82) Direct3D11 vs_5_0 ps_5_0, D3D11)",
    ),
    (
        "Google Inc. (Intel)",
        "ANGLE (Intel, Intel(R) UHD Graphics 630 (0x00003E9B) Direct3D11 vs_5_0 ps_5_0, D3D11)",
    ),
    (
        "Google Inc. (Intel)",
        "ANGLE (Intel, Intel(R) Iris(R) Xe Graphics (0x00009A49) Direct3D11 vs_5_0 ps_5_0, D3D11)",
    ),
    (
        "Google Inc. (AMD)",
        "ANGLE (AMD, AMD Radeon RX 6600 (0x000073FF) Direct3D11 vs_5_0 ps_5_0, D3D11)",
    ),
];
const WIN_LAPTOP_GPUS: &[(&str, &str)] = &[
    (
        "Google Inc. (Intel)",
        "ANGLE (Intel, Intel(R) Iris(R) Xe Graphics (0x00009A49) Direct3D11 vs_5_0 ps_5_0, D3D11)",
    ),
    (
        "Google Inc. (Intel)",
        "ANGLE (Intel, Intel(R) UHD Graphics (0x00009A60) Direct3D11 vs_5_0 ps_5_0, D3D11)",
    ),
    (
        "Google Inc. (AMD)",
        "ANGLE (AMD, AMD Radeon(TM) Graphics (0x00001681) Direct3D11 vs_5_0 ps_5_0, D3D11)",
    ),
];
/// UA-CH platformVersion of Windows 10 22H2 ("10.0.0"), Windows 11 23H2 ("15.0.0") and 24H2
/// ("19.0.0"). The UA string itself says "Windows NT 10.0" for all of them.
const WIN_VERSIONS: &[&str] = &["10.0.0", "15.0.0", "19.0.0", "19.0.0"];

const MAC_SILICON_GPUS: &[(&str, &str)] = &[
    (
        "Google Inc. (Apple)",
        "ANGLE (Apple, ANGLE Metal Renderer: Apple M1, Unspecified Version)",
    ),
    (
        "Google Inc. (Apple)",
        "ANGLE (Apple, ANGLE Metal Renderer: Apple M2, Unspecified Version)",
    ),
    (
        "Google Inc. (Apple)",
        "ANGLE (Apple, ANGLE Metal Renderer: Apple M3 Pro, Unspecified Version)",
    ),
];
const MAC_VERSIONS: &[&str] = &["14.6.1", "15.1.0", "15.3.1"];

const PIXEL_GPUS: &[(&str, &str)] = &[("ARM", "Mali-G715")];
const GALAXY_GPUS: &[(&str, &str)] = &[("Qualcomm", "Adreno (TM) 740")];
const ANDROID_VERSIONS: &[&str] = &["14.0.0", "15.0.0"];
const APPLE_GPUS: &[(&str, &str)] = &[("Apple Inc.", "Apple GPU")];
const IOS_VERSIONS: &[&str] = &["17_5", "18_1"];

/// Every preset. Desktop ones leave the viewport to the real window.
pub const PRESETS: &[Preset] = &[
    Preset {
        name: "windows-1080p",
        title: "Windows 10/11 desktop, Chrome, 1920x1080",
        device: None,
        os: Os::Windows,
        kind: Kind::Stealth,
        screen: Screen {
            width: 1920,
            height: 1080,
            dpr: 1.0,
        },
        viewport: None,
        cpus: &[4, 8, 12, 16],
        memory: &[8.0, 16.0, 32.0],
        gpus: WIN_GPUS,
        os_versions: WIN_VERSIONS,
        model: "",
        platform: "Win32",
        max_touch_points: 0,
        arch: "x86",
    },
    Preset {
        name: "windows-1440p",
        title: "Windows 10/11 desktop, Chrome, 2560x1440",
        device: None,
        os: Os::Windows,
        kind: Kind::Stealth,
        screen: Screen {
            width: 2560,
            height: 1440,
            dpr: 1.0,
        },
        viewport: None,
        cpus: &[8, 12, 16, 24],
        memory: &[16.0, 32.0],
        gpus: WIN_GPUS,
        os_versions: WIN_VERSIONS,
        model: "",
        platform: "Win32",
        max_touch_points: 0,
        arch: "x86",
    },
    Preset {
        name: "windows-laptop",
        title: "Windows 11 laptop, Chrome, 1920x1080 at 125% (1536x864)",
        device: None,
        os: Os::Windows,
        kind: Kind::Stealth,
        screen: Screen {
            width: 1536,
            height: 864,
            dpr: 1.25,
        },
        viewport: None,
        cpus: &[4, 8, 12],
        memory: &[8.0, 16.0],
        gpus: WIN_LAPTOP_GPUS,
        os_versions: &["15.0.0", "19.0.0"],
        model: "",
        platform: "Win32",
        max_touch_points: 0,
        arch: "x86",
    },
    Preset {
        name: "windows-768",
        title: "Windows 10 budget laptop, Chrome, 1366x768",
        device: None,
        os: Os::Windows,
        kind: Kind::Stealth,
        screen: Screen {
            width: 1366,
            height: 768,
            dpr: 1.0,
        },
        viewport: None,
        cpus: &[2, 4],
        memory: &[4.0, 8.0],
        gpus: WIN_LAPTOP_GPUS,
        os_versions: &["10.0.0", "15.0.0"],
        model: "",
        platform: "Win32",
        max_touch_points: 0,
        arch: "x86",
    },
    Preset {
        name: "macos-mbp14",
        title: "MacBook Pro 14\" (Apple Silicon), Chrome, 1512x982 @2x",
        device: None,
        os: Os::Macos,
        kind: Kind::Stealth,
        screen: Screen {
            width: 1512,
            height: 982,
            dpr: 2.0,
        },
        viewport: None,
        cpus: &[8, 10, 12],
        memory: &[16.0, 32.0],
        gpus: MAC_SILICON_GPUS,
        os_versions: MAC_VERSIONS,
        model: "",
        platform: "MacIntel",
        max_touch_points: 0,
        arch: "arm",
    },
    Preset {
        name: "macos-mba13",
        title: "MacBook Air 13\" (Apple Silicon), Chrome, 1440x900 @2x",
        device: None,
        os: Os::Macos,
        kind: Kind::Stealth,
        screen: Screen {
            width: 1440,
            height: 900,
            dpr: 2.0,
        },
        viewport: None,
        cpus: &[8],
        memory: &[8.0, 16.0],
        gpus: MAC_SILICON_GPUS,
        os_versions: MAC_VERSIONS,
        model: "",
        platform: "MacIntel",
        max_touch_points: 0,
        arch: "arm",
    },
    Preset {
        name: "android-pixel-8",
        title: "Google Pixel 8, Chrome for Android (mobile emulation on a desktop engine)",
        device: Some("Pixel 8"),
        os: Os::Android,
        kind: Kind::Stealth,
        screen: Screen {
            width: 412,
            height: 915,
            dpr: 2.625,
        },
        viewport: Some(Viewport {
            width: 412,
            height: 839,
        }),
        cpus: &[9],
        memory: &[8.0],
        gpus: PIXEL_GPUS,
        os_versions: ANDROID_VERSIONS,
        model: "Pixel 8",
        platform: "Linux armv81",
        max_touch_points: 5,
        arch: "",
    },
    Preset {
        name: "android-galaxy-s23",
        title: "Samsung Galaxy S23, Chrome for Android (mobile emulation on a desktop engine)",
        device: Some("Galaxy S23"),
        os: Os::Android,
        kind: Kind::Stealth,
        screen: Screen {
            width: 360,
            height: 780,
            dpr: 3.0,
        },
        viewport: Some(Viewport {
            width: 360,
            height: 704,
        }),
        cpus: &[8],
        memory: &[8.0],
        gpus: GALAXY_GPUS,
        os_versions: ANDROID_VERSIONS,
        model: "SM-S911B",
        platform: "Linux armv81",
        max_touch_points: 5,
        arch: "",
    },
    Preset {
        name: "iphone-15",
        title: "iPhone 15, Safari (device emulation only: the engine stays Chromium)",
        device: Some("iPhone 15"),
        os: Os::Ios,
        kind: Kind::DeviceEmulation,
        screen: Screen {
            width: 393,
            height: 852,
            dpr: 3.0,
        },
        viewport: Some(Viewport {
            width: 393,
            height: 659,
        }),
        cpus: &[4],
        memory: &[4.0],
        gpus: APPLE_GPUS,
        os_versions: IOS_VERSIONS,
        model: "iPhone",
        platform: "iPhone",
        max_touch_points: 5,
        arch: "",
    },
    Preset {
        name: "ipad-air",
        title: "iPad Air, Safari desktop mode (device emulation only: the engine stays Chromium)",
        device: Some("iPad"),
        os: Os::Ios,
        kind: Kind::DeviceEmulation,
        screen: Screen {
            width: 820,
            height: 1180,
            dpr: 2.0,
        },
        viewport: Some(Viewport {
            width: 820,
            height: 1106,
        }),
        cpus: &[8],
        memory: &[8.0],
        gpus: APPLE_GPUS,
        os_versions: IOS_VERSIONS,
        model: "iPad",
        // iPadOS Safari pretends to be a Mac.
        platform: "MacIntel",
        max_touch_points: 5,
        arch: "",
    },
];

/// Preset by key ("windows-1080p") or device name ("iPhone 15", case-insensitive).
pub fn preset(name: &str) -> Option<&'static Preset> {
    let wanted = name.trim().to_ascii_lowercase();
    PRESETS
        .iter()
        .find(|p| p.name == wanted || p.device.is_some_and(|d| d.to_ascii_lowercase() == wanted))
}

// ---------------------------------------------------------------- regions

/// Timezone, language and city that belong together (used for generated identities and to
/// suggest values for a proxy's country).
#[derive(Debug, Clone, Copy)]
pub struct Region {
    /// ISO country code, e.g. "DE".
    pub country: &'static str,
    pub timezone: &'static str,
    pub locale: &'static str,
    pub latitude: f64,
    pub longitude: f64,
}

pub const REGIONS: &[Region] = &[
    Region {
        country: "US",
        timezone: "America/New_York",
        locale: "en-US",
        latitude: 40.7128,
        longitude: -74.0060,
    },
    Region {
        country: "US",
        timezone: "America/Chicago",
        locale: "en-US",
        latitude: 41.8781,
        longitude: -87.6298,
    },
    Region {
        country: "US",
        timezone: "America/Los_Angeles",
        locale: "en-US",
        latitude: 34.0522,
        longitude: -118.2437,
    },
    Region {
        country: "CA",
        timezone: "America/Toronto",
        locale: "en-CA",
        latitude: 43.6532,
        longitude: -79.3832,
    },
    Region {
        country: "GB",
        timezone: "Europe/London",
        locale: "en-GB",
        latitude: 51.5074,
        longitude: -0.1278,
    },
    Region {
        country: "DE",
        timezone: "Europe/Berlin",
        locale: "de-DE",
        latitude: 52.5200,
        longitude: 13.4050,
    },
    Region {
        country: "FR",
        timezone: "Europe/Paris",
        locale: "fr-FR",
        latitude: 48.8566,
        longitude: 2.3522,
    },
    Region {
        country: "NL",
        timezone: "Europe/Amsterdam",
        locale: "nl-NL",
        latitude: 52.3676,
        longitude: 4.9041,
    },
    Region {
        country: "PL",
        timezone: "Europe/Warsaw",
        locale: "pl-PL",
        latitude: 52.2297,
        longitude: 21.0122,
    },
    Region {
        country: "UA",
        timezone: "Europe/Kyiv",
        locale: "uk-UA",
        latitude: 50.4501,
        longitude: 30.5234,
    },
    Region {
        country: "ES",
        timezone: "Europe/Madrid",
        locale: "es-ES",
        latitude: 40.4168,
        longitude: -3.7038,
    },
    Region {
        country: "IT",
        timezone: "Europe/Rome",
        locale: "it-IT",
        latitude: 41.9028,
        longitude: 12.4964,
    },
];

/// The usual locale for a country code ("de" → "de-DE"), for countries outside [`REGIONS`]
/// too (from the offline table in [`crate::country`]). `None` = unknown (the caller keeps its
/// default).
pub fn locale_for_country(country: &str) -> Option<&'static str> {
    let cc = country.trim().to_ascii_uppercase();
    if let Some(r) = REGIONS.iter().find(|r| r.country == cc) {
        return Some(r.locale);
    }
    crate::country::country(&cc).map(|c| c.languages[0])
}

/// The language list for a locale the way Chrome users usually have it: the locale, its
/// language, then English ("de-DE" → "de-DE,de,en-US,en").
///
/// No q-values: this list becomes `navigator.languages` as-is (verified live — with
/// "en;q=0.9" the page saw a language named "en;q=0.9"); Chromium adds the q-values to the
/// `Accept-Language` header itself.
pub fn accept_language(locale: &str) -> String {
    let lang = locale.split('-').next().unwrap_or(locale);
    if lang == "en" {
        if locale == "en-US" || locale == "en" {
            "en-US,en".to_string()
        } else {
            format!("{locale},en,en-US")
        }
    } else if lang == locale {
        format!("{locale},en-US,en")
    } else {
        format!("{locale},{lang},en-US,en")
    }
}

// ---------------------------------------------------------------- user agent strings

/// Chromium's GREASE brand for a major version (same algorithm as Chromium's
/// `GetGreasedUserAgentBrandVersion`), plus the order the three brands are listed in.
/// Example: 124 → "Not-A.Brand"/"99", listed as [Chromium, Google Chrome, Not-A.Brand].
pub fn chrome_brands(major: u32, full: &str) -> (Vec<Brand>, Vec<Brand>) {
    const CHARS: [char; 11] = [' ', '(', ':', '-', '.', '/', ')', ';', '=', '?', '_'];
    const VERSIONS: [&str; 3] = ["8", "99", "24"];
    const ORDERS: [[usize; 3]; 6] = [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ];
    let seed = major as usize;
    let grease = format!(
        "Not{}A{}Brand",
        CHARS[seed % CHARS.len()],
        CHARS[(seed + 1) % CHARS.len()]
    );
    let grease_version = VERSIONS[seed % VERSIONS.len()];
    let order = ORDERS[seed % ORDERS.len()];
    // (brand, major version, full version)
    let items = [
        (
            grease,
            grease_version.to_string(),
            format!("{grease_version}.0.0.0"),
        ),
        ("Chromium".to_string(), major.to_string(), full.to_string()),
        (
            "Google Chrome".to_string(),
            major.to_string(),
            full.to_string(),
        ),
    ];
    // `order[i]` is the position of item i.
    let mut slots: Vec<(usize, &(String, String, String))> = items
        .iter()
        .enumerate()
        .map(|(i, it)| (order[i], it))
        .collect();
    slots.sort_by_key(|(pos, _)| *pos);
    let brands = slots
        .iter()
        .map(|(_, (b, v, _))| Brand {
            brand: b.clone(),
            version: v.clone(),
        })
        .collect();
    let full_list = slots
        .iter()
        .map(|(_, (b, _, f))| Brand {
            brand: b.clone(),
            version: f.clone(),
        })
        .collect();
    (brands, full_list)
}

/// Reduced Chrome UA strings (Chrome freezes the OS part since UA reduction).
fn user_agent(p: &Preset, major: u32, os_version: &str) -> String {
    let chrome = format!("Chrome/{major}.0.0.0");
    match p.os {
        Os::Windows => format!(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) \
             {chrome} Safari/537.36"
        ),
        Os::Macos => format!(
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like \
             Gecko) {chrome} Safari/537.36"
        ),
        Os::Android => format!(
            "Mozilla/5.0 (Linux; Android 10; K) AppleWebKit/537.36 (KHTML, like Gecko) {chrome} \
             Mobile Safari/537.36"
        ),
        Os::Ios => {
            let safari = os_version.replace('_', ".");
            if p.model == "iPad" {
                format!(
                    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 \
                     (KHTML, like Gecko) Version/{safari} Safari/605.1.15"
                )
            } else {
                format!(
                    "Mozilla/5.0 (iPhone; CPU iPhone OS {os_version} like Mac OS X) \
                     AppleWebKit/605.1.15 (KHTML, like Gecko) Version/{safari} Mobile/15E148 \
                     Safari/604.1"
                )
            }
        }
    }
}

fn ua_metadata(p: &Preset, major: u32, os_version: &str) -> Option<UaMetadata> {
    let platform = match p.os {
        Os::Windows => "Windows",
        Os::Macos => "macOS",
        Os::Android => "Android",
        // Safari sends no client hints.
        Os::Ios => return None,
    };
    let full = format!("{major}.0.0.0");
    let (brands, full_version_list) = chrome_brands(major, &full);
    let mobile = p.os == Os::Android;
    Some(UaMetadata {
        brands,
        full_version_list,
        platform: platform.to_string(),
        platform_version: os_version.to_string(),
        architecture: p.arch.to_string(),
        model: p.model.to_string(),
        mobile,
        bitness: if mobile { String::new() } else { "64".into() },
        wow64: false,
    })
}

// ---------------------------------------------------------------- building identities

/// Optional overrides for [`generate`] / [`from_preset`] (e.g. from `geo.lookup` of a proxy).
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Overrides {
    /// IANA timezone, e.g. "Europe/Berlin".
    pub timezone: Option<String>,
    /// Locale, e.g. "de-DE" (Accept-Language is derived from it).
    pub locale: Option<String>,
    /// Country code to pick timezone/locale/location from, e.g. "DE".
    pub country: Option<String>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
}

/// The preset as-is: first choice of every list, US East region.
pub fn from_preset(p: &Preset, id: &str) -> Identity {
    build(p, id, &mut Rng::new(0), true, &Overrides::default())
}

/// A random identity (deterministic for a seed). `os` = None picks Windows or macOS
/// (desktop Chrome), weighted towards Windows like real traffic.
pub fn generate(os: Option<Os>, seed: u64, overrides: &Overrides) -> Identity {
    let mut rng = Rng::new(seed);
    let os = os.unwrap_or_else(|| {
        if rng.below(10) < 7 {
            Os::Windows
        } else {
            Os::Macos
        }
    });
    let candidates: Vec<&Preset> = PRESETS.iter().filter(|p| p.os == os).collect();
    let preset = *rng.pick(&candidates);
    build(preset, &default_id(os, seed), &mut rng, false, overrides)
}

/// A random variant of one preset (CPU, memory, GPU, OS version, region), deterministic for
/// a seed.
pub fn generate_preset(preset: &Preset, seed: u64, overrides: &Overrides) -> Identity {
    let mut rng = Rng::new(seed);
    build(
        preset,
        &default_id(preset.os, seed),
        &mut rng,
        false,
        overrides,
    )
}

/// "win-0000002a" for Windows and seed 42.
fn default_id(os: Os, seed: u64) -> String {
    let short = match os {
        Os::Windows => "win",
        Os::Macos => "mac",
        Os::Android => "android",
        Os::Ios => "ios",
    };
    format!("{short}-{:08x}", seed as u32)
}

fn build(p: &Preset, id: &str, rng: &mut Rng, first: bool, ov: &Overrides) -> Identity {
    let mut choose = |n: usize| if first { 0 } else { rng.below(n) };
    let os_version = p.os_versions[choose(p.os_versions.len())];
    let cpus = p.cpus[choose(p.cpus.len())];
    let memory = p.memory[choose(p.memory.len())];
    let (vendor, renderer) = p.gpus[choose(p.gpus.len())];

    // Region: explicit country, else random (first = US East).
    let wanted_country = ov.country.as_deref().map(str::to_ascii_uppercase);
    let in_country: Vec<&Region> = REGIONS
        .iter()
        .filter(|r| wanted_country.as_deref().is_none_or(|c| r.country == c))
        .collect();
    let region = if in_country.is_empty() {
        REGIONS[0]
    } else {
        *in_country[choose(in_country.len())]
    };
    // Up to ~5 km off the city centre, so not every identity sits on the same point.
    let jitter = |rng_value: usize| (rng_value as f64 / 1000.0 - 0.5) * 0.09;
    let (dlat, dlon) = if first {
        (0.0, 0.0)
    } else {
        (jitter(choose(1000)), jitter(choose(1000)))
    };
    let accuracy = if first {
        50.0
    } else {
        20.0 + choose(130) as f64
    };

    let mut locale = region.locale.to_string();
    if let Some(cc) = &wanted_country
        && region.country != cc
        && let Some(l) = locale_for_country(cc)
    {
        locale = l.to_string();
    }
    if let Some(l) = &ov.locale {
        locale = l.clone();
    }
    let timezone = ov
        .timezone
        .clone()
        .unwrap_or_else(|| region.timezone.to_string());
    let latitude = ov.latitude.unwrap_or(region.latitude + dlat);
    let longitude = ov.longitude.unwrap_or(region.longitude + dlon);

    let major = ENGINE_CHROME_MAJOR;
    let mobile = matches!(p.os, Os::Android | Os::Ios) && p.model != "iPad";
    Identity {
        id: id.to_string(),
        preset: p.name.to_string(),
        kind: p.kind,
        os: p.os,
        user_agent: user_agent(p, major, os_version),
        ua_metadata: ua_metadata(p, major, os_version),
        platform: p.platform.to_string(),
        hardware_concurrency: cpus,
        device_memory: memory,
        screen: p.screen,
        viewport: p.viewport,
        timezone,
        accept_language: accept_language(&locale),
        locale,
        geolocation: Some(Geolocation {
            latitude: (latitude * 1e4).round() / 1e4,
            longitude: (longitude * 1e4).round() / 1e4,
            accuracy,
        }),
        touch: p.max_touch_points > 0,
        max_touch_points: p.max_touch_points,
        mobile,
        webgl_vendor: vendor.to_string(),
        webgl_renderer: renderer.to_string(),
    }
}

impl Identity {
    /// Use the running engine's real full version (e.g. "156.0.7632.45") in the client hints
    /// when its major matches the claimed one. Returns a warning when it does not match.
    pub fn adapt_to_engine(&mut self, engine_full: &str) -> Option<String> {
        let engine_major = engine_full.split('.').next().unwrap_or_default();
        let claimed = chrome_major(&self.user_agent)?;
        if claimed != engine_major {
            return Some(format!(
                "identity claims Chrome {claimed} but the engine is Chrome {engine_full}: \
                 feature detection can reveal the real version"
            ));
        }
        if let Some(meta) = &mut self.ua_metadata {
            for b in &mut meta.full_version_list {
                if b.brand == "Chromium" || b.brand == "Google Chrome" {
                    b.version = engine_full.to_string();
                }
            }
        }
        None
    }
}

/// "156" from "... Chrome/156.0.0.0 ...".
pub fn chrome_major(ua: &str) -> Option<&str> {
    let rest = ua.split("Chrome/").nth(1)?;
    rest.split('.').next()
}

/// Everything inconsistent about an identity, as readable sentences (empty = consistent).
pub fn problems(id: &Identity) -> Vec<String> {
    let mut out = Vec::new();
    let ua = &id.user_agent;
    let mut bad = |msg: String| out.push(msg);

    // UA string ↔ claimed OS ↔ navigator.platform.
    let (ua_token, platforms): (&str, &[&str]) = match id.os {
        Os::Windows => ("Windows NT", &["Win32"]),
        Os::Macos => ("Macintosh", &["MacIntel"]),
        Os::Android => (
            "Android",
            &["Linux armv81", "Linux armv8l", "Linux aarch64"],
        ),
        Os::Ios => ("like Mac OS X", &["iPhone", "iPad", "MacIntel"]),
    };
    let ipad_desktop_mode = id.os == Os::Ios && ua.contains("Macintosh");
    if !ua.contains(ua_token) && !ipad_desktop_mode {
        bad(format!(
            "user agent does not mention {ua_token} for os {}",
            id.os.as_str()
        ));
    }
    if !platforms.contains(&id.platform.as_str()) {
        bad(format!(
            "navigator.platform {:?} does not fit os {}",
            id.platform,
            id.os.as_str()
        ));
    }
    if ua.contains("HeadlessChrome") {
        bad("user agent says HeadlessChrome".into());
    }

    // UA ↔ client hints.
    let ua_mobile = ua.contains("Mobile");
    match (&id.ua_metadata, chrome_major(ua)) {
        (Some(meta), Some(major)) => {
            let chrome = meta.brands.iter().find(|b| b.brand == "Google Chrome");
            match chrome {
                Some(b) if b.version == major => {}
                Some(b) => bad(format!(
                    "UA says Chrome {major} but client hints say {}",
                    b.version
                )),
                None => bad("client hints have no \"Google Chrome\" brand".into()),
            }
            let full_ok = meta
                .full_version_list
                .iter()
                .filter(|b| b.brand == "Google Chrome" || b.brand == "Chromium")
                .all(|b| b.version.split('.').next() == Some(major));
            if !full_ok {
                bad("fullVersionList major differs from the UA".into());
            }
            let expected_platform = match id.os {
                Os::Windows => "Windows",
                Os::Macos => "macOS",
                Os::Android => "Android",
                Os::Ios => "iOS",
            };
            if meta.platform != expected_platform {
                bad(format!(
                    "client hints platform {:?}, expected {expected_platform:?}",
                    meta.platform
                ));
            }
            if meta.mobile != ua_mobile {
                bad("client hints mobile flag differs from the UA's Mobile token".into());
            }
            if id.os == Os::Android && meta.model.is_empty() {
                bad("Android client hints need a model".into());
            }
            if id.os != Os::Android && !meta.model.is_empty() {
                bad("desktop client hints must have an empty model".into());
            }
        }
        (Some(_), None) => bad("client hints given but the UA is not Chrome".into()),
        (None, Some(_)) => bad("a Chrome UA needs client hints".into()),
        (None, None) => {}
    }
    if id.mobile != ua_mobile {
        bad("mobile flag differs from the UA's Mobile token".into());
    }

    // Hardware.
    if !DEVICE_MEMORY_VALUES.contains(&id.device_memory) {
        bad(format!(
            "deviceMemory {} is not a value Chrome reports",
            id.device_memory
        ));
    }
    if id.hardware_concurrency == 0 || id.hardware_concurrency > 64 {
        bad(format!(
            "hardwareConcurrency {} is implausible",
            id.hardware_concurrency
        ));
    }
    if id.touch != (id.max_touch_points > 0) {
        bad("touch flag and maxTouchPoints disagree".into());
    }
    if matches!(id.os, Os::Android | Os::Ios) && !id.touch {
        bad("a phone/tablet needs touch".into());
    }
    if matches!(id.os, Os::Windows | Os::Macos) && id.mobile {
        bad("a desktop OS cannot be mobile".into());
    }
    if let Some(vp) = id.viewport
        && (vp.width > id.screen.width || vp.height > id.screen.height)
    {
        bad("viewport is larger than the screen".into());
    }
    if id.screen.dpr <= 0.0 {
        bad("device pixel ratio must be positive".into());
    }

    // GPU ↔ OS.
    let gpu = &id.webgl_renderer;
    let gpu_ok = match id.os {
        Os::Windows => gpu.contains("Direct3D") || gpu.contains("D3D"),
        Os::Macos => gpu.contains("Metal") || gpu.contains("OpenGL"),
        Os::Android => !gpu.contains("Direct3D") && !gpu.contains("ANGLE"),
        Os::Ios => gpu.contains("Apple"),
    };
    if !gpu_ok {
        bad(format!(
            "WebGL renderer {gpu:?} does not exist on {}",
            id.os.as_str()
        ));
    }

    // Language and time.
    if !id.accept_language.starts_with(&id.locale) {
        bad("Accept-Language does not start with the locale".into());
    }
    if id.accept_language.contains(';') {
        bad("Accept-Language must be a plain list (no ;q=): it becomes navigator.languages".into());
    }
    if !id.timezone.contains('/') && id.timezone != "UTC" {
        bad(format!("timezone {:?} is not an IANA zone", id.timezone));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grease_matches_real_chrome() {
        // Chrome 124 sends: "Chromium";v="124", "Google Chrome";v="124", "Not-A.Brand";v="99".
        let (brands, _) = chrome_brands(124, "124.0.6367.60");
        let got: Vec<(&str, &str)> = brands
            .iter()
            .map(|b| (b.brand.as_str(), b.version.as_str()))
            .collect();
        assert_eq!(
            got,
            [
                ("Chromium", "124"),
                ("Google Chrome", "124"),
                ("Not-A.Brand", "99")
            ]
        );
        // The engine version: deterministic, three distinct brands.
        let (brands, full) = chrome_brands(156, "156.0.1.2");
        assert_eq!(brands.len(), 3);
        assert!(
            full.iter()
                .any(|b| b.brand == "Google Chrome" && b.version == "156.0.1.2")
        );
    }

    #[test]
    fn every_preset_is_consistent() {
        for p in PRESETS {
            let id = from_preset(p, "t");
            let problems = problems(&id);
            assert!(problems.is_empty(), "{}: {problems:?}", p.name);
            assert_eq!(
                id.kind == Kind::DeviceEmulation,
                p.os == Os::Ios,
                "{}",
                p.name
            );
        }
    }

    #[test]
    fn generated_identities_are_consistent() {
        for os in [
            None,
            Some(Os::Windows),
            Some(Os::Macos),
            Some(Os::Android),
            Some(Os::Ios),
        ] {
            for seed in 0..200 {
                let id = generate(os, seed, &Overrides::default());
                let problems = problems(&id);
                assert!(problems.is_empty(), "{os:?}/{seed}: {problems:?} {id:#?}");
                if let Some(os) = os {
                    assert_eq!(id.os, os);
                }
            }
        }
    }

    #[test]
    fn windows_identity_fits_together() {
        let id = generate(Some(Os::Windows), 42, &Overrides::default());
        assert!(id.user_agent.contains("Windows NT 10.0; Win64; x64"));
        assert!(
            id.user_agent
                .contains(&format!("Chrome/{ENGINE_CHROME_MAJOR}.0.0.0"))
        );
        let meta = id.ua_metadata.as_ref().unwrap();
        assert_eq!(meta.platform, "Windows");
        assert_eq!(meta.architecture, "x86");
        assert_eq!(meta.bitness, "64");
        assert_eq!(id.platform, "Win32");
        assert!(id.webgl_renderer.contains("Direct3D11"));
        assert!(!id.mobile && !id.touch && id.viewport.is_none());
    }

    #[test]
    fn generator_is_deterministic() {
        let ov = Overrides::default();
        assert_eq!(generate(None, 7, &ov), generate(None, 7, &ov));
        assert_eq!(
            generate(Some(Os::Macos), 99, &ov),
            generate(Some(Os::Macos), 99, &ov)
        );
        // Different seeds give different identities (at least somewhere in 20 tries).
        let first = generate(Some(Os::Windows), 1, &ov);
        assert!((2..22).any(|s| {
            let other = generate(Some(Os::Windows), s, &ov);
            (other.screen, other.webgl_renderer, other.timezone)
                != (
                    first.screen,
                    first.webgl_renderer.clone(),
                    first.timezone.clone(),
                )
        }));
    }

    #[test]
    fn overrides_win() {
        let ov = Overrides {
            country: Some("de".into()),
            ..Overrides::default()
        };
        let id = generate(Some(Os::Windows), 5, &ov);
        assert_eq!(id.timezone, "Europe/Berlin");
        assert_eq!(id.locale, "de-DE");
        assert_eq!(id.accept_language, "de-DE,de,en-US,en");

        let ov = Overrides {
            country: Some("JP".into()),
            timezone: Some("Asia/Tokyo".into()),
            latitude: Some(35.68),
            longitude: Some(139.69),
            ..Overrides::default()
        };
        let id = generate(Some(Os::Windows), 5, &ov);
        assert_eq!(
            (id.timezone.as_str(), id.locale.as_str()),
            ("Asia/Tokyo", "ja-JP")
        );
        assert_eq!(id.geolocation.unwrap().latitude, 35.68);
        assert!(problems(&id).is_empty());
    }

    #[test]
    fn engine_version_is_adopted_when_majors_match() {
        let mut id = from_preset(preset("windows-1080p").unwrap(), "t");
        assert_eq!(id.adapt_to_engine("156.0.7632.45"), None);
        let meta = id.ua_metadata.as_ref().unwrap();
        assert!(
            meta.full_version_list
                .iter()
                .any(|b| b.brand == "Google Chrome" && b.version == "156.0.7632.45")
        );
        assert!(id.adapt_to_engine("150.0.1.1").is_some());
    }

    #[test]
    fn problems_catch_mismatches() {
        let mut id = from_preset(preset("windows-1080p").unwrap(), "t");
        id.platform = "MacIntel".into();
        id.webgl_renderer =
            "ANGLE (Apple, ANGLE Metal Renderer: Apple M1, Unspecified Version)".into();
        id.device_memory = 12.0;
        let p = problems(&id);
        assert_eq!(p.len(), 3, "{p:?}");
    }

    #[test]
    fn presets_by_device_name() {
        assert_eq!(preset("iPhone 15").unwrap().name, "iphone-15");
        assert_eq!(preset("pixel 8").unwrap().name, "android-pixel-8");
        assert_eq!(preset("ipad").unwrap().name, "ipad-air");
        assert!(preset("Nokia 3310").is_none());
        let iphone = from_preset(preset("iPhone 15").unwrap(), "t");
        assert!(iphone.ua_metadata.is_none());
        assert!(iphone.user_agent.contains("iPhone OS 17_5"));
        assert!(iphone.mobile && iphone.touch);
        let ipad = from_preset(preset("iPad").unwrap(), "t");
        assert!(!ipad.mobile && ipad.touch && ipad.user_agent.contains("Macintosh"));
    }

    #[test]
    fn accept_language_forms() {
        assert_eq!(accept_language("en-US"), "en-US,en");
        assert_eq!(accept_language("en-GB"), "en-GB,en,en-US");
        assert_eq!(accept_language("uk-UA"), "uk-UA,uk,en-US,en");
        assert_eq!(locale_for_country("br"), Some("pt-BR"));
        assert_eq!(locale_for_country("ZZ"), None);
    }
}
