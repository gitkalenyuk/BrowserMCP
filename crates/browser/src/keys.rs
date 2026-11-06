//! Keyboard chords like `"Enter"`, `"Control+A"`, `"Shift+Tab"` → CDP `Input.dispatchKeyEvent`s.
//!
//! A *chord* is modifiers plus one key joined with `+`. Several chords can be given separated by
//! spaces: `"Tab Tab Enter"`. Key names follow the web `KeyboardEvent.key` values (`Enter`,
//! `ArrowDown`, `PageUp`, `a`, `7`, `F5`, …); `Ctrl`, `Cmd`, `Esc`, `Del`, `Up`/`Down`… and
//! `Space` are accepted as aliases.

use bmcp_core::{CmdError, Ctx, TabId};
use serde_json::{Value, json};

use crate::cdp;

/// One key with everything CDP needs to make the page see a real key press.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Key {
    /// `KeyboardEvent.key`, e.g. `"Enter"` or `"a"`.
    pub key: String,
    /// `KeyboardEvent.code`, e.g. `"Enter"` or `"KeyA"`.
    pub code: String,
    /// Windows virtual key code (CDP `windowsVirtualKeyCode`), e.g. 13 for Enter.
    pub key_code: u32,
    /// Text the key types (`"\r"` for Enter, `"a"`), if any.
    pub text: Option<String>,
}

/// A parsed chord: modifier bit mask (CDP: Alt=1, Ctrl=2, Meta=4, Shift=8) and the main key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chord {
    pub modifiers: u32,
    /// Modifier keys in the order they are pressed.
    pub modifier_keys: Vec<Key>,
    pub key: Key,
}

const ALT: u32 = 1;
const CTRL: u32 = 2;
const META: u32 = 4;
const SHIFT: u32 = 8;

fn key(key: &str, code: &str, key_code: u32, text: Option<&str>) -> Key {
    Key {
        key: key.to_string(),
        code: code.to_string(),
        key_code,
        text: text.map(str::to_string),
    }
}

/// Modifier name → (bit, key).
fn modifier(name: &str) -> Option<(u32, Key)> {
    Some(match name.to_ascii_lowercase().as_str() {
        "alt" | "option" => (ALT, key("Alt", "AltLeft", 18, None)),
        "control" | "ctrl" => (CTRL, key("Control", "ControlLeft", 17, None)),
        "meta" | "cmd" | "command" | "win" | "super" => (META, key("Meta", "MetaLeft", 91, None)),
        "shift" => (SHIFT, key("Shift", "ShiftLeft", 16, None)),
        _ => return None,
    })
}

/// Named (non-character) key → key.
fn named(name: &str) -> Option<Key> {
    let lower = name.to_ascii_lowercase();
    let k = match lower.as_str() {
        "enter" | "return" => key("Enter", "Enter", 13, Some("\r")),
        "tab" => key("Tab", "Tab", 9, None),
        "escape" | "esc" => key("Escape", "Escape", 27, None),
        "backspace" => key("Backspace", "Backspace", 8, None),
        "delete" | "del" => key("Delete", "Delete", 46, None),
        "space" | " " => key(" ", "Space", 32, Some(" ")),
        "arrowup" | "up" => key("ArrowUp", "ArrowUp", 38, None),
        "arrowdown" | "down" => key("ArrowDown", "ArrowDown", 40, None),
        "arrowleft" | "left" => key("ArrowLeft", "ArrowLeft", 37, None),
        "arrowright" | "right" => key("ArrowRight", "ArrowRight", 39, None),
        "home" => key("Home", "Home", 36, None),
        "end" => key("End", "End", 35, None),
        "pageup" => key("PageUp", "PageUp", 33, None),
        "pagedown" => key("PageDown", "PageDown", 34, None),
        "insert" => key("Insert", "Insert", 45, None),
        _ => {
            // F1..F12
            let n: u32 = lower.strip_prefix('f')?.parse().ok()?;
            if !(1..=12).contains(&n) {
                return None;
            }
            let name = format!("F{n}");
            return Some(key(&name, &name, 111 + n, None));
        }
    };
    Some(k)
}

/// A single printable character → key (`a` → KeyA/65, `7` → Digit7/55, others by text only).
fn character(c: char, shift: bool) -> Key {
    // "Control+A" means the A key: lower-case text unless Shift is held.
    let text = if !c.is_ascii_alphabetic() {
        c.to_string()
    } else if shift {
        c.to_ascii_uppercase().to_string()
    } else {
        c.to_ascii_lowercase().to_string()
    };
    if c.is_ascii_alphabetic() {
        let upper = c.to_ascii_uppercase();
        return key(&text, &format!("Key{upper}"), upper as u32, Some(&text));
    }
    if c.is_ascii_digit() {
        return key(&text, &format!("Digit{c}"), c as u32, Some(&text));
    }
    // Punctuation and non-ASCII: the page sees the right `key`/text; the code is unknown.
    key(&text, "", 0, Some(&text))
}

/// US-keyboard key for a punctuation character: (code, virtual key code, needs Shift).
fn punctuation_key(c: char) -> Option<(&'static str, u32, bool)> {
    Some(match c {
        '.' => ("Period", 190, false),
        ',' => ("Comma", 188, false),
        '-' => ("Minus", 189, false),
        '=' => ("Equal", 187, false),
        '/' => ("Slash", 191, false),
        ';' => ("Semicolon", 186, false),
        '\'' => ("Quote", 222, false),
        '[' => ("BracketLeft", 219, false),
        ']' => ("BracketRight", 221, false),
        '\\' => ("Backslash", 220, false),
        '`' => ("Backquote", 192, false),
        '>' => ("Period", 190, true),
        '<' => ("Comma", 188, true),
        '_' => ("Minus", 189, true),
        '+' => ("Equal", 187, true),
        '?' => ("Slash", 191, true),
        ':' => ("Semicolon", 186, true),
        '"' => ("Quote", 222, true),
        '{' => ("BracketLeft", 219, true),
        '}' => ("BracketRight", 221, true),
        '|' => ("Backslash", 220, true),
        '~' => ("Backquote", 192, true),
        '!' => ("Digit1", 49, true),
        '@' => ("Digit2", 50, true),
        '#' => ("Digit3", 51, true),
        '$' => ("Digit4", 52, true),
        '%' => ("Digit5", 53, true),
        '^' => ("Digit6", 54, true),
        '&' => ("Digit7", 55, true),
        '*' => ("Digit8", 56, true),
        '(' => ("Digit9", 57, true),
        ')' => ("Digit0", 48, true),
        _ => return None,
    })
}

/// The chord a person presses to type `c` on a US keyboard (`A` = Shift+KeyA, `!` =
/// Shift+Digit1), or `None` for characters without a key (accents, emoji, newline): those are
/// inserted as text instead.
pub fn char_chord(c: char) -> Option<Chord> {
    let shift_key = || {
        modifier("shift")
            .map(|(_, k)| k)
            .into_iter()
            .collect::<Vec<_>>()
    };
    if c == ' ' {
        return Some(Chord {
            modifiers: 0,
            modifier_keys: Vec::new(),
            key: named("space")?,
        });
    }
    if c.is_ascii_alphanumeric() {
        let shift = c.is_ascii_uppercase();
        return Some(Chord {
            modifiers: if shift { SHIFT } else { 0 },
            modifier_keys: if shift { shift_key() } else { Vec::new() },
            key: character(c, shift),
        });
    }
    let (code, key_code, shift) = punctuation_key(c)?;
    Some(Chord {
        modifiers: if shift { SHIFT } else { 0 },
        modifier_keys: if shift { shift_key() } else { Vec::new() },
        key: key(&c.to_string(), code, key_code, Some(&c.to_string())),
    })
}

/// Parse one chord such as `"Control+Shift+K"`.
pub fn parse_chord(input: &str) -> Result<Chord, CmdError> {
    let bad = |why: &str| {
        CmdError::invalid_params(format!("bad key chord '{input}': {why}")).with_hint(
            "Use names like Enter, Tab, Escape, ArrowDown, a, F5, joined with + for modifiers: \
             Control+A, Shift+Tab.",
        )
    };
    // "Control++" means Control and the "+" key: split off a trailing "+" key first.
    let (head, last) = match input.strip_suffix("++") {
        Some(head) => (head, "+"),
        None => match input.rsplit_once('+') {
            Some((head, last)) => (head, last),
            None => ("", input),
        },
    };
    let mut modifiers = 0;
    let mut modifier_keys = Vec::new();
    for part in head.split('+').filter(|p| !p.is_empty()) {
        let (bit, key) =
            modifier(part).ok_or_else(|| bad(&format!("unknown modifier '{part}'")))?;
        modifiers |= bit;
        modifier_keys.push(key);
    }
    if last.is_empty() {
        return Err(bad("missing key after '+'"));
    }
    let key = if let Some(k) = named(last) {
        k
    } else if let Some((_, k)) = modifier(last) {
        // A lone modifier press, e.g. "Shift".
        k
    } else {
        let mut chars = last.chars();
        match (chars.next(), chars.next()) {
            (Some(c), None) => character(c, modifiers & SHIFT != 0),
            _ => return Err(bad(&format!("unknown key '{last}'"))),
        }
    };
    Ok(Chord {
        modifiers,
        modifier_keys,
        key,
    })
}

/// Parse `"Tab Tab Enter"` (chords separated by spaces).
pub fn parse_keys(input: &str) -> Result<Vec<Chord>, CmdError> {
    let chords: Vec<Chord> = input
        .split_whitespace()
        .map(parse_chord)
        .collect::<Result<_, _>>()?;
    if chords.is_empty() {
        return Err(CmdError::invalid_params("no keys given").with_hint("e.g. keys: \"Enter\""));
    }
    Ok(chords)
}

/// Editing command Chrome should run for well-known shortcuts. Key events alone do not
/// trigger them reliably when the window is in the background, so CDP lets us name them.
fn editing_command(chord: &Chord) -> Option<&'static str> {
    if chord.modifiers & (CTRL | META) == 0 || chord.modifiers & ALT != 0 {
        return None;
    }
    Some(match chord.key.code.as_str() {
        "KeyA" => "selectAll",
        "KeyC" => "copy",
        "KeyX" => "cut",
        "KeyV" => "paste",
        "KeyZ" if chord.modifiers & SHIFT != 0 => "redo",
        "KeyZ" => "undo",
        "KeyY" => "redo",
        _ => return None,
    })
}

/// The `Input.dispatchKeyEvent` parameters for one chord, in order (modifier downs, key down,
/// key up, modifier ups). Separate from sending so it can be unit-tested.
pub fn chord_events(chord: &Chord) -> Vec<Value> {
    let mut events = Vec::new();
    let mut held = 0;
    for m in &chord.modifier_keys {
        held |= modifier(&m.key).map(|(bit, _)| bit).unwrap_or(0);
        events.push(key_event("rawKeyDown", m, held, None, None));
    }
    // Text is typed only without Ctrl/Alt/Meta (Ctrl+A selects, it does not type "a").
    let types_text = chord.modifiers & (CTRL | ALT | META) == 0;
    let text = chord.key.text.as_deref().filter(|_| types_text);
    let down = if text.is_some() {
        "keyDown"
    } else {
        "rawKeyDown"
    };
    let command = editing_command(chord);
    events.push(key_event(down, &chord.key, chord.modifiers, text, command));
    events.push(key_event("keyUp", &chord.key, chord.modifiers, None, None));
    for m in chord.modifier_keys.iter().rev() {
        held &= !modifier(&m.key).map(|(bit, _)| bit).unwrap_or(0);
        events.push(key_event("keyUp", m, held, None, None));
    }
    events
}

fn key_event(
    kind: &str,
    key: &Key,
    modifiers: u32,
    text: Option<&str>,
    command: Option<&str>,
) -> Value {
    let mut event = json!({
        "type": kind,
        "key": key.key,
        "code": key.code,
        "windowsVirtualKeyCode": key.key_code,
        "modifiers": modifiers,
    });
    if let Some(text) = text {
        event["text"] = json!(text);
        event["unmodifiedText"] = json!(text);
    }
    if let Some(command) = command {
        event["commands"] = json!([command]);
    }
    event
}

/// Send chords to whatever has keyboard focus in the tab.
pub async fn press(ctx: &Ctx, tab: TabId, chords: &[Chord]) -> Result<(), CmdError> {
    for chord in chords {
        for event in chord_events(chord) {
            cdp::call(ctx, tab, "Input.dispatchKeyEvent", event).await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_named_keys_and_modifiers() {
        let enter = parse_chord("Enter").unwrap();
        assert_eq!(enter.modifiers, 0);
        assert_eq!(enter.key.key_code, 13);
        assert_eq!(enter.key.text.as_deref(), Some("\r"));

        let select_all = parse_chord("Control+A").unwrap();
        assert_eq!(select_all.modifiers, CTRL);
        assert_eq!(select_all.key.code, "KeyA");

        let back_tab = parse_chord("shift+tab").unwrap();
        assert_eq!(back_tab.modifiers, SHIFT);
        assert_eq!(back_tab.key.key, "Tab");

        assert_eq!(parse_chord("ArrowDown").unwrap().key.key_code, 40);
        assert_eq!(parse_chord("F5").unwrap().key.key_code, 116);
        assert_eq!(parse_chord("Ctrl++").unwrap().key.key, "+");
        assert_eq!(parse_keys("Tab Tab Enter").unwrap().len(), 3);
    }

    #[test]
    fn rejects_unknown_keys() {
        assert!(parse_chord("Hyper+A").is_err());
        assert!(parse_chord("Control+").is_err());
        assert!(parse_chord("Bogus").is_err());
        assert!(parse_keys("   ").is_err());
    }

    #[test]
    fn control_a_sends_select_all_without_text() {
        let events = chord_events(&parse_chord("Control+A").unwrap());
        let kinds: Vec<&str> = events.iter().map(|e| e["type"].as_str().unwrap()).collect();
        assert_eq!(kinds, ["rawKeyDown", "rawKeyDown", "keyUp", "keyUp"]);
        assert_eq!(events[0]["key"], "Control");
        assert_eq!(events[1]["key"], "a");
        assert_eq!(events[1]["modifiers"], CTRL);
        assert_eq!(events[1]["commands"], json!(["selectAll"]));
        assert!(events[1].get("text").is_none());
        assert_eq!(events[3]["modifiers"], 0);
    }

    #[test]
    fn characters_map_to_us_keyboard_chords() {
        let events = chord_events(&char_chord('A').unwrap());
        let summary: Vec<(String, String, i64)> = events
            .iter()
            .map(|e| {
                (
                    e["type"].as_str().unwrap().to_string(),
                    e["key"].as_str().unwrap().to_string(),
                    e["modifiers"].as_i64().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                ("rawKeyDown".to_string(), "Shift".to_string(), 8),
                ("keyDown".to_string(), "A".to_string(), 8),
                ("keyUp".to_string(), "A".to_string(), 8),
                ("keyUp".to_string(), "Shift".to_string(), 0),
            ]
        );
        assert_eq!(events[1]["text"], "A");
        assert_eq!(events[1]["code"], "KeyA");

        let a = chord_events(&char_chord('a').unwrap());
        assert_eq!(a.len(), 2);
        assert_eq!(
            (a[0]["code"].clone(), a[0]["windowsVirtualKeyCode"].clone()),
            (json!("KeyA"), json!(65))
        );
        let bang = char_chord('!').unwrap();
        assert_eq!((bang.key.code.as_str(), bang.modifiers), ("Digit1", SHIFT));
        assert_eq!(char_chord('.').unwrap().key.key_code, 190);
        assert_eq!(char_chord(' ').unwrap().key.code, "Space");
        assert!(char_chord('é').is_none());
        assert!(char_chord('\n').is_none());
    }

    #[test]
    fn enter_types_carriage_return() {
        let events = chord_events(&parse_chord("Enter").unwrap());
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["type"], "keyDown");
        assert_eq!(events[0]["text"], "\r");
        assert_eq!(events[1]["type"], "keyUp");
    }
}
