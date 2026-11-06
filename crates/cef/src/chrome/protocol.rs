//! Pure (CEF-free, unit-tested) parts of the spike-S1 chrome: the UI → browser command
//! channel, omnibox URL fix-up, keyboard shortcuts and palette geometry.
//!
//! ## The spike command channel
//!
//! * UI page → browser: `fetch("bmcp://cmd/<name>?<args>")`. The `bmcp` scheme handler
//!   (browser process, IO thread) parses the URL with [`parse_command`], answers `204` at
//!   once and posts the command to the UI thread, where it is executed only if the sender
//!   is one of our chrome views (a web page could also fetch that URL; it is ignored).
//! * Browser → UI page: `frame.execute_java_script("bmcp.onState({...})")`, see
//!   [`state_script`].
//!
//! Phase 3 replaces this with the real WS JSON-RPC API.

/// A command sent by the UI page (or the palette page).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// The page loaded and wants the current content state pushed to it.
    Ready,
    /// Navigate the content view. The text is what the user typed (fixed up by us).
    Navigate(String),
    Back,
    Forward,
    Reload,
    Stop,
    Palette(PaletteAction),
    /// Give keyboard focus back to the page.
    FocusContent,
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
    Some(match name.trim_end_matches('/') {
        "ready" => Command::Ready,
        "navigate" => Command::Navigate(arg("url")?),
        "back" => Command::Back,
        "forward" => Command::Forward,
        "reload" => Command::Reload,
        "stop" => Command::Stop,
        "focus-content" => Command::FocusContent,
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

/// Turn omnibox text into a URL: keep URLs, add a scheme to host-like text, search the
/// rest. Returns `None` for empty input.
pub fn fixup_url(input: &str) -> Option<String> {
    let text = input.trim();
    if text.is_empty() {
        return None;
    }
    let has_scheme = text.contains("://")
        || ["about:", "data:", "javascript:", "view-source:"]
            .iter()
            .any(|p| text.starts_with(p));
    if has_scheme {
        return Some(text.to_owned());
    }
    let host = text.split(['/', '?', '#']).next().unwrap_or(text);
    let host_name = host.split(':').next().unwrap_or(host);
    if !text.contains(char::is_whitespace) {
        if host_name == "localhost" || host_name == "127.0.0.1" {
            return Some(format!("http://{text}"));
        }
        if host_name.contains('.') && !host_name.starts_with('.') && !host_name.ends_with('.') {
            return Some(format!("https://{text}"));
        }
    }
    Some(format!(
        "https://duckduckgo.com/?q={}",
        percent_encode(text)
    ))
}

/// Browser-wide shortcuts that work no matter which view (UI, page, palette) has focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shortcut {
    TogglePalette,
    FocusOmnibox,
    Reload,
    Back,
    Forward,
    /// Esc: closes the palette (only consumed while the palette is open).
    Escape,
}

// Windows virtual key codes (VK_*); CEF passes them in `KeyEvent::windows_key_code`.
const VK_ESCAPE: i32 = 0x1B;
const VK_LEFT: i32 = 0x25;
const VK_RIGHT: i32 = 0x27;
const VK_K: i32 = 0x4B;
const VK_L: i32 = 0x4C;
const VK_R: i32 = 0x52;
const VK_F5: i32 = 0x74;

/// Modifier keys held during a key press.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

/// Map a key-down to a shortcut (Chrome-compatible bindings).
pub fn shortcut(key_code: i32, m: Modifiers) -> Option<Shortcut> {
    let plain = !m.ctrl && !m.alt && !m.shift;
    let ctrl_only = m.ctrl && !m.alt && !m.shift;
    let alt_only = m.alt && !m.ctrl && !m.shift;
    match key_code {
        VK_K if ctrl_only => Some(Shortcut::TogglePalette),
        VK_L if ctrl_only => Some(Shortcut::FocusOmnibox),
        VK_R if ctrl_only => Some(Shortcut::Reload),
        VK_F5 if plain => Some(Shortcut::Reload),
        VK_LEFT if alt_only => Some(Shortcut::Back),
        VK_RIGHT if alt_only => Some(Shortcut::Forward),
        VK_ESCAPE if plain => Some(Shortcut::Escape),
        _ => None,
    }
}

/// A rectangle in window coordinates (DIP).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Bounds {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// Preferred palette size (DIP).
pub const PALETTE_SIZE: (i32, i32) = (600, 400);
/// Minimum gap between the palette and the edges of the content area (DIP).
const PALETTE_MARGIN: i32 = 16;

/// Centre the palette over the content area, shrinking it when the window is small.
pub fn palette_bounds(content: Bounds) -> Bounds {
    let fit = |want: i32, room: i32| want.min(room - 2 * PALETTE_MARGIN).max(0);
    let width = fit(PALETTE_SIZE.0, content.width);
    let height = fit(PALETTE_SIZE.1, content.height);
    Bounds {
        x: content.x + (content.width - width) / 2,
        y: content.y + (content.height - height) / 2,
        width,
        height,
    }
}

/// State of the content view, pushed to the UI page.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContentState {
    pub url: String,
    pub title: String,
    pub loading: bool,
    pub can_go_back: bool,
    pub can_go_forward: bool,
}

/// JavaScript that hands `state` to the UI page (`window.bmcp.onState`).
pub fn state_script(state: &ContentState) -> String {
    let json = serde_json::json!({
        "url": state.url,
        "title": state.title,
        "loading": state.loading,
        "canGoBack": state.can_go_back,
        "canGoForward": state.can_go_forward,
    });
    format!("window.bmcp && window.bmcp.onState({json});")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_commands() {
        assert_eq!(parse_command("bmcp://cmd/ready"), Some(Command::Ready));
        assert_eq!(parse_command("bmcp://cmd/back/"), Some(Command::Back));
        assert_eq!(
            parse_command("bmcp://cmd/navigate?url=https%3A%2F%2Fa.test%2Fx%3Fq%3D1"),
            Some(Command::Navigate("https://a.test/x?q=1".into()))
        );
        assert_eq!(
            parse_command("bmcp://cmd/palette?action=hide"),
            Some(Command::Palette(PaletteAction::Hide))
        );
        assert_eq!(
            parse_command("bmcp://cmd/palette"),
            Some(Command::Palette(PaletteAction::Toggle))
        );
        assert_eq!(parse_command("bmcp://cmd/navigate"), None);
        assert_eq!(parse_command("bmcp://cmd/rm-rf"), None);
        assert_eq!(parse_command("https://cmd/back"), None);
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
    fn fixes_up_urls() {
        assert_eq!(fixup_url("  "), None);
        assert_eq!(
            fixup_url("https://a.test").as_deref(),
            Some("https://a.test")
        );
        assert_eq!(fixup_url("about:blank").as_deref(), Some("about:blank"));
        assert_eq!(
            fixup_url("example.com/path").as_deref(),
            Some("https://example.com/path")
        );
        assert_eq!(
            fixup_url("localhost:5173").as_deref(),
            Some("http://localhost:5173")
        );
        assert_eq!(
            fixup_url("rust borrow checker").as_deref(),
            Some("https://duckduckgo.com/?q=rust%20borrow%20checker")
        );
        assert_eq!(
            fixup_url("hello").as_deref(),
            Some("https://duckduckgo.com/?q=hello")
        );
    }

    #[test]
    fn maps_shortcuts() {
        let ctrl = Modifiers {
            ctrl: true,
            ..Default::default()
        };
        let alt = Modifiers {
            alt: true,
            ..Default::default()
        };
        assert_eq!(shortcut(0x4B, ctrl), Some(Shortcut::TogglePalette));
        assert_eq!(shortcut(0x4B, Modifiers::default()), None);
        assert_eq!(shortcut(0x4C, ctrl), Some(Shortcut::FocusOmnibox));
        assert_eq!(shortcut(0x74, Modifiers::default()), Some(Shortcut::Reload));
        assert_eq!(shortcut(0x25, alt), Some(Shortcut::Back));
        assert_eq!(shortcut(0x27, alt), Some(Shortcut::Forward));
        assert_eq!(shortcut(0x25, Modifiers::default()), None);
        assert_eq!(shortcut(0x1B, Modifiers::default()), Some(Shortcut::Escape));
    }

    #[test]
    fn palette_is_centred_and_shrinks() {
        let content = Bounds {
            x: 0,
            y: 80,
            width: 1280,
            height: 720,
        };
        assert_eq!(
            palette_bounds(content),
            Bounds {
                x: 340,
                y: 240,
                width: 600,
                height: 400
            }
        );
        let small = Bounds {
            x: 0,
            y: 80,
            width: 300,
            height: 200,
        };
        let b = palette_bounds(small);
        assert_eq!((b.width, b.height), (268, 168));
        assert_eq!((b.x, b.y), (16, 96));
    }

    #[test]
    fn state_script_is_valid_js_call() {
        let script = state_script(&ContentState {
            url: "https://a.test/\"</script>".into(),
            title: "T".into(),
            loading: true,
            ..Default::default()
        });
        assert!(script.starts_with("window.bmcp && window.bmcp.onState({"));
        assert!(script.contains(r#""url":"https://a.test/\"</script>""#));
        assert!(script.contains(r#""canGoBack":false"#));
    }
}
