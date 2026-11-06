//! Pure (CEF-free, unit-tested) parts of the composed window: the small `bmcp://cmd/`
//! channel, keyboard shortcuts, page-area insets and palette geometry.
//!
//! ## Who talks how
//!
//! * Tabs, navigation, titles, favicons: the Svelte UI uses the WS JSON-RPC API
//!   (`tabs.*`, `nav.*`, `tab.*` events) like any other client.
//! * Things that only make sense for "the window this page is in" go through
//!   `fetch("bmcp://cmd/<name>?<args>")`, because the scheme handler knows which browser
//!   sent the request and therefore which window it belongs to:
//!   `layout` (where the page area is), `palette?action=show|hide|toggle`,
//!   `focus-content`, `focus-omnibox`. The handler (IO thread) parses the URL with
//!   [`parse_command`], answers `204` at once and posts the command to the UI thread,
//!   where it runs only if the sender is one of our own UI/palette views.
//! * Browser → UI page: `execute_java_script("window.bmcp.run('tab.new')")` for shortcuts
//!   pressed while a web page has focus, `bmcp.onShow()` / `bmcp.onHide()` for the
//!   palette overlay.

/// A command sent by the UI page (or the palette page) over `bmcp://cmd/`.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// Where the UI wants the web page: the page slot's rectangle in its viewport.
    Layout(PageArea),
    Palette(PaletteAction),
    /// Give keyboard focus to the page (e.g. after Enter in the omnibox).
    FocusContent,
    /// Give keyboard focus to the UI view and its omnibox (from the palette).
    FocusOmnibox,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaletteAction {
    Show,
    Hide,
    Toggle,
}

/// Prefix of every command URL.
pub const CMD_PREFIX: &str = "bmcp://cmd/";

/// Parse `bmcp://cmd/<name>?<query>`. Returns `None` for anything unknown.
pub fn parse_command(url: &str) -> Option<Command> {
    let rest = url.strip_prefix(CMD_PREFIX)?;
    let (name, query) = rest.split_once('?').unwrap_or((rest, ""));
    let arg = |key: &str| {
        query
            .split('&')
            .filter_map(|pair| pair.split_once('='))
            .find(|(k, _)| *k == key)
            .map(|(_, v)| percent_decode(v))
    };
    let number = |key: &str| {
        arg(key)
            .and_then(|v| v.parse::<f64>().ok())
            .filter(|v| v.is_finite())
    };
    Some(match name.trim_end_matches('/') {
        "layout" => Command::Layout(PageArea {
            x: number("x")?,
            y: number("y")?,
            w: number("w")?,
            h: number("h")?,
            vw: number("vw")?,
            vh: number("vh")?,
        }),
        "focus-content" => Command::FocusContent,
        "focus-omnibox" => Command::FocusOmnibox,
        "palette" => Command::Palette(match arg("action").as_deref() {
            Some("show") => PaletteAction::Show,
            Some("hide") => PaletteAction::Hide,
            _ => PaletteAction::Toggle,
        }),
        _ => return None,
    })
}

/// Decode `%XX` escapes and `+` (space) from a URL query value. Invalid escapes are kept
/// as they are; invalid UTF-8 is replaced with U+FFFD.
pub fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        // Value of the two hex digits after a '%', if they are there and valid.
        let escaped = (bytes[i] == b'%')
            .then(|| bytes.get(i + 1..i + 3))
            .flatten()
            .filter(|hex| hex.iter().all(u8::is_ascii_hexdigit))
            .and_then(|hex| std::str::from_utf8(hex).ok())
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match (bytes[i], escaped) {
            (_, Some(byte)) => {
                out.push(byte);
                i += 3;
                continue;
            }
            (b'+', None) => out.push(b' '),
            (b, None) => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Percent-encode everything except unreserved characters (RFC 3986).
pub fn percent_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

// ------------------------------------------------------------------ shortcuts

/// What a key press in a web page (or in the palette) means for the browser.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shortcut {
    /// Ctrl+K: show/hide the palette overlay (native, no round trip).
    TogglePalette,
    /// Ctrl+L / Alt+D / F6: focus the UI view, then its omnibox.
    FocusOmnibox,
    /// Esc: closes the palette (only consumed while the palette is open).
    Escape,
    /// Ctrl+F: focus the UI view, then open its find bar (`find.open`).
    Find,
    /// Any other browser shortcut: run this UI command id (`ui/src/lib/keymap.ts`) in the
    /// window's UI page, exactly as if the key had been pressed there.
    Ui(&'static str),
}

// Windows virtual key codes (VK_*); CEF passes them in `KeyEvent::windows_key_code`.
const VK_TAB: i32 = 0x09;
const VK_ESCAPE: i32 = 0x1B;
const VK_PRIOR: i32 = 0x21; // Page Up
const VK_NEXT: i32 = 0x22; // Page Down
const VK_LEFT: i32 = 0x25;
const VK_RIGHT: i32 = 0x27;
const VK_0: i32 = 0x30;
const VK_D: i32 = 0x44;
const VK_E: i32 = 0x45;
const VK_F: i32 = 0x46;
const VK_K: i32 = 0x4B;
const VK_L: i32 = 0x4C;
const VK_R: i32 = 0x52;
const VK_T: i32 = 0x54;
const VK_W: i32 = 0x57;
const VK_F4: i32 = 0x73;
const VK_F5: i32 = 0x74;
const VK_F6: i32 = 0x75;
/// The `` ` `` key on a US layout.
const VK_OEM_3: i32 = 0xC0;

/// Modifier keys held during a key press.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

/// Map a key-down in a web page to a browser shortcut.
///
/// Mirrors `DEFAULT_KEYMAP` in `ui/src/lib/keymap.ts` (Chrome-compatible bindings). Keys
/// pressed in the UI view itself are handled by the UI page, so they never come here.
/// TODO(keymap.json): once user overrides exist, the UI should send its effective keymap.
pub fn shortcut(key_code: i32, m: Modifiers) -> Option<Shortcut> {
    let plain = !m.ctrl && !m.alt && !m.shift;
    let ctrl = m.ctrl && !m.alt && !m.shift;
    let ctrl_shift = m.ctrl && m.shift && !m.alt;
    let alt = m.alt && !m.ctrl && !m.shift;
    use Shortcut::Ui;
    Some(match key_code {
        VK_K if ctrl => Shortcut::TogglePalette,
        VK_L if ctrl => Shortcut::FocusOmnibox,
        VK_D if alt => Shortcut::FocusOmnibox,
        VK_F6 if plain => Shortcut::FocusOmnibox,
        VK_ESCAPE if plain => Shortcut::Escape,
        VK_F if ctrl => Shortcut::Find,
        VK_T if ctrl => Ui("tab.new"),
        VK_T if ctrl_shift => Ui("tab.reopen"),
        VK_W | VK_F4 if ctrl => Ui("tab.close"),
        VK_TAB | VK_NEXT if ctrl => Ui("tab.next"),
        VK_TAB if ctrl_shift => Ui("tab.prev"),
        VK_PRIOR if ctrl => Ui("tab.prev"),
        0x31..=0x38 if ctrl => Ui(SELECT[(key_code - VK_0 - 1) as usize]),
        0x39 if ctrl => Ui("tab.selectLast"),
        VK_LEFT if alt => Ui("nav.back"),
        VK_RIGHT if alt => Ui("nav.forward"),
        VK_R if ctrl => Ui("nav.reload"),
        VK_F5 if plain => Ui("nav.reload"),
        VK_E if ctrl_shift => Ui("ui.toggleVerticalTabs"),
        0x31..=0x36 if ctrl_shift => Ui(PANELS[(key_code - VK_0 - 1) as usize]),
        VK_OEM_3 if ctrl => Ui("panel.terminal"),
        _ => return None,
    })
}

const SELECT: [&str; 8] = [
    "tab.select1",
    "tab.select2",
    "tab.select3",
    "tab.select4",
    "tab.select5",
    "tab.select6",
    "tab.select7",
    "tab.select8",
];

/// Ctrl+Shift+1..6 (`panel.terminal` also has Ctrl+`).
const PANELS: [&str; 6] = [
    "panel.traffic",
    "panel.recording",
    "panel.ai",
    "panel.terminal",
    "panel.scripts",
    "panel.runs",
];

/// JavaScript that runs a UI command id in the chrome page. `id` comes from our own table.
pub fn run_script(id: &str) -> String {
    format!("window.bmcp && window.bmcp.run({});", serde_json::json!(id))
}

// ------------------------------------------------------------------ geometry

/// A rectangle in window coordinates (DIP).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Bounds {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// The UI's page slot as reported over `bmcp://cmd/layout` (CSS px = DIP at zoom 100 %):
/// its rectangle `x, y, w, h` inside a viewport of `vw × vh`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageArea {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub vw: f64,
    pub vh: f64,
}

/// Distance of the page area from each window edge (DIP). The window keeps these when
/// it is resized, so the page follows a resize at once, without waiting for the UI page
/// to re-report its layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Insets {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Insets {
    /// Before the UI reported anything: tab strip (34) + toolbar (40) of the Svelte chrome.
    pub const DEFAULT: Insets = Insets {
        left: 0,
        top: 74,
        right: 0,
        bottom: 0,
    };

    pub fn from_area(a: PageArea) -> Insets {
        let px = |v: f64| v.round().max(0.0) as i32;
        Insets {
            left: px(a.x),
            top: px(a.y),
            right: px(a.vw - a.x - a.w),
            bottom: px(a.vh - a.y - a.h),
        }
    }
}

/// Where the page goes in a window client area of `width × height`.
pub fn page_bounds(width: i32, height: i32, insets: Insets) -> Bounds {
    Bounds {
        x: insets.left,
        y: insets.top,
        width: (width - insets.left - insets.right).max(0),
        height: (height - insets.top - insets.bottom).max(0),
    }
}

/// Preferred palette size (DIP); the Svelte palette fills its view.
pub const PALETTE_SIZE: (i32, i32) = (620, 420);
/// Minimum gap between the palette and the edges of the page area (DIP).
const PALETTE_MARGIN: i32 = 12;

/// Centre the palette horizontally near the top of the page area, shrinking it when the
/// window is small (like Chrome's omnibox dropdown / VS Code's palette).
pub fn palette_bounds(page: Bounds) -> Bounds {
    let fit = |want: i32, room: i32| want.min(room - 2 * PALETTE_MARGIN).max(0);
    let width = fit(PALETTE_SIZE.0, page.width);
    let height = fit(PALETTE_SIZE.1, page.height);
    Bounds {
        x: page.x + (page.width - width) / 2,
        y: page.y + PALETTE_MARGIN.min((page.height - height).max(0)),
        width,
        height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_commands() {
        assert_eq!(
            parse_command("bmcp://cmd/focus-content"),
            Some(Command::FocusContent)
        );
        assert_eq!(
            parse_command("bmcp://cmd/focus-omnibox/"),
            Some(Command::FocusOmnibox)
        );
        assert_eq!(
            parse_command("bmcp://cmd/palette?action=hide"),
            Some(Command::Palette(PaletteAction::Hide))
        );
        assert_eq!(
            parse_command("bmcp://cmd/palette"),
            Some(Command::Palette(PaletteAction::Toggle))
        );
        assert_eq!(
            parse_command("bmcp://cmd/layout?x=0&y=84.4&w=1280&h=716&vw=1280&vh=800"),
            Some(Command::Layout(PageArea {
                x: 0.0,
                y: 84.4,
                w: 1280.0,
                h: 716.0,
                vw: 1280.0,
                vh: 800.0
            }))
        );
        // Missing / bad numbers, removed spike commands, other schemes.
        assert_eq!(parse_command("bmcp://cmd/layout?x=0&y=1"), None);
        assert_eq!(
            parse_command("bmcp://cmd/layout?x=NaN&y=0&w=1&h=1&vw=1&vh=1"),
            None
        );
        assert_eq!(parse_command("bmcp://cmd/navigate?url=x"), None);
        assert_eq!(parse_command("https://cmd/palette"), None);
    }

    #[test]
    fn percent_round_trip() {
        assert_eq!(percent_decode("a+b%20c%D0%BF"), "a b cп");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%zz%4"), "%zz%4");
        assert_eq!(percent_encode("a b/п"), "a%20b%2F%D0%BF");
        assert_eq!(percent_decode(&percent_encode("x=1&y=ü")), "x=1&y=ü");
    }

    #[test]
    fn maps_shortcuts() {
        let ctrl = Modifiers {
            ctrl: true,
            ..Default::default()
        };
        let ctrl_shift = Modifiers {
            ctrl: true,
            shift: true,
            ..Default::default()
        };
        let alt = Modifiers {
            alt: true,
            ..Default::default()
        };
        let none = Modifiers::default();
        assert_eq!(shortcut(0x4B, ctrl), Some(Shortcut::TogglePalette));
        assert_eq!(shortcut(0x4B, none), None);
        assert_eq!(shortcut(0x4C, ctrl), Some(Shortcut::FocusOmnibox));
        assert_eq!(shortcut(0x44, alt), Some(Shortcut::FocusOmnibox));
        assert_eq!(shortcut(0x1B, none), Some(Shortcut::Escape));
        assert_eq!(shortcut(0x46, ctrl), Some(Shortcut::Find));
        assert_eq!(shortcut(0x54, ctrl), Some(Shortcut::Ui("tab.new")));
        assert_eq!(shortcut(0x54, ctrl_shift), Some(Shortcut::Ui("tab.reopen")));
        assert_eq!(shortcut(0x57, ctrl), Some(Shortcut::Ui("tab.close")));
        assert_eq!(shortcut(0x09, ctrl), Some(Shortcut::Ui("tab.next")));
        assert_eq!(shortcut(0x09, ctrl_shift), Some(Shortcut::Ui("tab.prev")));
        assert_eq!(shortcut(0x31, ctrl), Some(Shortcut::Ui("tab.select1")));
        assert_eq!(shortcut(0x38, ctrl), Some(Shortcut::Ui("tab.select8")));
        assert_eq!(shortcut(0x39, ctrl), Some(Shortcut::Ui("tab.selectLast")));
        assert_eq!(shortcut(0x33, ctrl_shift), Some(Shortcut::Ui("panel.ai")));
        assert_eq!(shortcut(0x37, ctrl_shift), None);
        assert_eq!(shortcut(0x74, none), Some(Shortcut::Ui("nav.reload")));
        assert_eq!(shortcut(0x52, ctrl), Some(Shortcut::Ui("nav.reload")));
        assert_eq!(shortcut(0x25, alt), Some(Shortcut::Ui("nav.back")));
        assert_eq!(shortcut(0x27, alt), Some(Shortcut::Ui("nav.forward")));
        // Plain letters and arrows belong to the page.
        assert_eq!(shortcut(0x25, none), None);
        assert_eq!(shortcut(0x54, none), None);
        assert_eq!(
            run_script("tab.new"),
            r#"window.bmcp && window.bmcp.run("tab.new");"#
        );
    }

    #[test]
    fn insets_follow_the_page_slot() {
        let insets = Insets::from_area(PageArea {
            x: 240.0,
            y: 74.4,
            w: 660.0,
            h: 725.6,
            vw: 1280.0,
            vh: 800.0,
        });
        assert_eq!(
            insets,
            Insets {
                left: 240,
                top: 74,
                right: 380,
                bottom: 0
            }
        );
        // The same insets in a bigger window: the page grows, the chrome stays.
        assert_eq!(
            page_bounds(1600, 1000, insets),
            Bounds {
                x: 240,
                y: 74,
                width: 980,
                height: 926
            }
        );
        // Never negative.
        assert_eq!(page_bounds(100, 50, insets).width, 0);
        assert_eq!(page_bounds(100, 50, insets).height, 0);
    }

    #[test]
    fn palette_is_centred_near_the_top_and_shrinks() {
        let page = Bounds {
            x: 0,
            y: 74,
            width: 1280,
            height: 726,
        };
        assert_eq!(
            palette_bounds(page),
            Bounds {
                x: 330,
                y: 86,
                width: 620,
                height: 420
            }
        );
        let small = Bounds {
            x: 0,
            y: 74,
            width: 300,
            height: 200,
        };
        let b = palette_bounds(small);
        assert_eq!((b.width, b.height), (276, 176));
        assert_eq!((b.x, b.y), (12, 86));
    }
}
