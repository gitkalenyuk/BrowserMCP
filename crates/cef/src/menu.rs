//! The context menu of web pages: CEF's native menu (it is drawn over the page by the OS, so
//! it is never hidden under a view) filled with our own model.
//!
//! * [`build`] decides the entries from what was clicked (pure, unit-tested);
//! * [`fill`] puts them into CEF's `MenuModel` (`on_before_context_menu`);
//! * [`run`] does our own commands (`on_context_menu_command`). CEF's built-in ids (Back,
//!   Forward, Reload, Undo … Select all) are executed by CEF itself.
//!
//! Our commands: open link in a new tab / in another workspace, copy link address, save
//! image (into `data/downloads`, through the download handler), open image in a new tab,
//! copy image address, "Ask AI about this" (`ui.ask_ai` on the bus: the AI panel picks it
//! up), Inspect (DevTools in a window of its own).

use std::cell::RefCell;

use bmcp_core::TabId;
use cef::*;
use serde_json::json;

use crate::bridge::{PendingTab, emit, with_ui};
use crate::profiles::ProfileKey;

// CEF's built-in command ids (`cef_menu_id_t`), executed by CEF.
pub(crate) const BACK: i32 = 100;
pub(crate) const FORWARD: i32 = 101;
pub(crate) const RELOAD: i32 = 102;
pub(crate) const UNDO: i32 = 110;
pub(crate) const REDO: i32 = 111;
pub(crate) const CUT: i32 = 112;
pub(crate) const COPY: i32 = 113;
pub(crate) const PASTE: i32 = 114;
pub(crate) const SELECT_ALL: i32 = 117;
pub(crate) const PRINT: i32 = 131;
/// `MENU_ID_USER_FIRST`: our own ids start here.
pub(crate) const USER_FIRST: i32 = 26500;

pub(crate) const OPEN_LINK_NEW_TAB: i32 = USER_FIRST;
pub(crate) const COPY_LINK: i32 = USER_FIRST + 1;
pub(crate) const SAVE_IMAGE: i32 = USER_FIRST + 2;
pub(crate) const OPEN_IMAGE_NEW_TAB: i32 = USER_FIRST + 3;
pub(crate) const COPY_IMAGE_URL: i32 = USER_FIRST + 4;
pub(crate) const ASK_AI: i32 = USER_FIRST + 5;
pub(crate) const INSPECT: i32 = USER_FIRST + 6;
/// Id of the "Open link in workspace" submenu itself.
pub(crate) const WORKSPACE_MENU: i32 = USER_FIRST + 50;
/// `WORKSPACE_FIRST + i` = open the link in workspace `i` of [`MenuFacts::workspaces`].
pub(crate) const WORKSPACE_FIRST: i32 = USER_FIRST + 100;

/// What was right-clicked, as far as the menu cares.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct MenuFacts {
    pub link: Option<String>,
    /// Source URL of a clicked image.
    pub image: Option<String>,
    pub selection: Option<String>,
    pub editable: bool,
    pub can_back: bool,
    pub can_forward: bool,
    /// Other workspaces (profile id, label) a link can be opened in.
    pub workspaces: Vec<(String, String)>,
}

/// One menu line.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Entry {
    Item {
        id: i32,
        label: String,
        enabled: bool,
    },
    Separator,
    Submenu {
        id: i32,
        label: String,
        items: Vec<(i32, String)>,
    },
}

fn item(id: i32, label: &str) -> Entry {
    Entry::Item {
        id,
        label: label.to_string(),
        enabled: true,
    }
}

/// Shorten a selection for the menu label ("Ask AI about “…”").
pub(crate) fn short(text: &str, max: usize) -> String {
    let one_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() <= max {
        one_line
    } else {
        let cut: String = one_line.chars().take(max.saturating_sub(1)).collect();
        format!("{cut}…")
    }
}

/// The menu for `facts` (pure). Groups are separated; there is never a separator at the
/// start, at the end or twice in a row.
pub(crate) fn build(facts: &MenuFacts) -> Vec<Entry> {
    let mut groups: Vec<Vec<Entry>> = Vec::new();
    if facts.link.is_some() {
        let mut g = vec![item(OPEN_LINK_NEW_TAB, "Open link in new tab")];
        if !facts.workspaces.is_empty() {
            g.push(Entry::Submenu {
                id: WORKSPACE_MENU,
                label: "Open link in workspace".into(),
                items: facts
                    .workspaces
                    .iter()
                    .enumerate()
                    .map(|(i, (_, label))| (WORKSPACE_FIRST + i as i32, label.clone()))
                    .collect(),
            });
        }
        g.push(item(COPY_LINK, "Copy link address"));
        groups.push(g);
    }
    if facts.image.is_some() {
        groups.push(vec![
            item(OPEN_IMAGE_NEW_TAB, "Open image in new tab"),
            item(SAVE_IMAGE, "Save image (to Downloads)"),
            item(COPY_IMAGE_URL, "Copy image address"),
        ]);
    }
    if facts.editable {
        groups.push(vec![
            item(UNDO, "Undo"),
            item(REDO, "Redo"),
            Entry::Separator,
            item(CUT, "Cut"),
            item(COPY, "Copy"),
            item(PASTE, "Paste"),
            item(SELECT_ALL, "Select all"),
        ]);
    } else if facts.selection.is_some() {
        groups.push(vec![item(COPY, "Copy")]);
    }
    let nothing_special = facts.link.is_none() && facts.image.is_none() && !facts.editable;
    if nothing_special && facts.selection.is_none() {
        groups.push(vec![
            Entry::Item {
                id: BACK,
                label: "Back".into(),
                enabled: facts.can_back,
            },
            Entry::Item {
                id: FORWARD,
                label: "Forward".into(),
                enabled: facts.can_forward,
            },
            item(RELOAD, "Reload"),
            item(PRINT, "Print…"),
        ]);
    }
    let ask = match (&facts.selection, &facts.link) {
        (Some(text), _) => format!("Ask AI about “{}”", short(text, 32)),
        (None, Some(_)) => "Ask AI about this link".into(),
        (None, None) if facts.image.is_some() => "Ask AI about this image".into(),
        _ => "Ask AI about this page".into(),
    };
    groups.push(vec![Entry::Item {
        id: ASK_AI,
        label: ask,
        enabled: true,
    }]);
    groups.push(vec![item(INSPECT, "Inspect")]);

    let mut out = Vec::new();
    for g in groups.into_iter().filter(|g| !g.is_empty()) {
        if !out.is_empty() {
            out.push(Entry::Separator);
        }
        out.extend(g);
    }
    out
}

/// What `ui.ask_ai` carries for these facts: `(kind, text)`.
pub(crate) fn ask_payload(facts: &MenuFacts, page_url: &str) -> (&'static str, String) {
    if let Some(text) = &facts.selection {
        ("selection", text.clone())
    } else if let Some(link) = &facts.link {
        ("link", link.clone())
    } else if let Some(image) = &facts.image {
        ("image", image.clone())
    } else {
        ("page", page_url.to_string())
    }
}

// ------------------------------------------------------------------ CEF glue

/// The menu that is open: browser id, what was clicked, where (for Inspect).
type OpenMenu = (i32, MenuFacts, (i32, i32));

thread_local! {
    /// Facts of the menu that is open, for `on_context_menu_command`.
    static OPEN: RefCell<Option<OpenMenu>> = const { RefCell::new(None) };
}

fn text(s: CefStringUserfree) -> String {
    CefString::from(&s).to_string()
}

/// Read what was clicked from CEF's params.
pub(crate) fn facts(browser: &Browser, params: &ContextMenuParams) -> MenuFacts {
    let non_empty = |s: String| (!s.is_empty()).then_some(s);
    let link = non_empty(text(params.link_url()));
    let image = (params.media_type() == ContextMenuMediaType::IMAGE)
        .then(|| text(params.source_url()))
        .and_then(non_empty);
    let selection = non_empty(text(params.selection_text())).filter(|s| !s.trim().is_empty());
    let id = TabId(browser.identifier());
    let (can_back, can_forward, profile) = with_ui(|ui| {
        ui.tabs.get(&id).map_or((false, false, String::new()), |t| {
            (t.can_go_back, t.can_go_forward, t.profile.name())
        })
    });
    MenuFacts {
        link,
        image,
        selection,
        editable: params.is_editable() == 1,
        can_back,
        can_forward,
        workspaces: crate::profiles::workspaces()
            .into_iter()
            .filter(|(id, _)| *id != profile)
            .collect(),
    }
}

/// `on_before_context_menu` of a web page: replace CEF's model with ours.
pub(crate) fn fill(browser: &Browser, params: &ContextMenuParams, model: &MenuModel) {
    let facts = facts(browser, params);
    model.clear();
    for entry in build(&facts) {
        match entry {
            Entry::Separator => {
                model.add_separator();
            }
            Entry::Item { id, label, enabled } => {
                model.add_item(id, Some(&CefString::from(label.as_str())));
                if !enabled {
                    model.set_enabled(id, 0);
                }
            }
            Entry::Submenu { id, label, items } => {
                if let Some(sub) = model.add_sub_menu(id, Some(&CefString::from(label.as_str()))) {
                    for (id, label) in items {
                        sub.add_item(id, Some(&CefString::from(label.as_str())));
                    }
                }
            }
        }
    }
    let point = (params.xcoord(), params.ycoord());
    OPEN.with(|open| *open.borrow_mut() = Some((browser.identifier(), facts, point)));
}

/// `on_context_menu_command`: returns 1 when it was one of ours.
pub(crate) fn run(browser: &Browser, command: i32) -> i32 {
    if command < USER_FIRST {
        return 0; // CEF's own (Back, Copy, …)
    }
    let open = OPEN.with(|open| open.borrow_mut().take());
    let Some((owner, facts, point)) = open.filter(|(id, _, _)| *id == browser.identifier()) else {
        return 1;
    };
    let id = TabId(owner);
    match command {
        OPEN_LINK_NEW_TAB => {
            if let Some(link) = &facts.link {
                open_next_to(id, link, None);
            }
        }
        OPEN_IMAGE_NEW_TAB => {
            if let Some(image) = &facts.image {
                open_next_to(id, image, None);
            }
        }
        COPY_LINK => copy(facts.link.as_deref()),
        COPY_IMAGE_URL => copy(facts.image.as_deref()),
        SAVE_IMAGE => {
            if let (Some(image), Some(host)) = (&facts.image, browser.host()) {
                // The download handler saves it into data/downloads, without a dialog.
                host.start_download(Some(&CefString::from(image.as_str())));
            }
        }
        ASK_AI => {
            let (url, title) = with_ui(|ui| {
                ui.tabs
                    .get(&id)
                    .map(|t| (t.url.clone(), t.title.clone()))
                    .unwrap_or_default()
            });
            let (kind, text) = ask_payload(&facts, &url);
            emit(
                "ui.ask_ai",
                json!({ "tab": id, "url": url, "title": title, "kind": kind, "text": text }),
            );
        }
        INSPECT => crate::devtools::show(browser, point),
        c if c >= WORKSPACE_FIRST => {
            let index = (c - WORKSPACE_FIRST) as usize;
            if let (Some(link), Some((profile, _))) = (&facts.link, facts.workspaces.get(index)) {
                open_next_to(id, link, Some(profile.clone()));
            }
        }
        _ => {}
    }
    1
}

/// An off-screen (automation) tab has no screen to show a menu on: the menu is logged and
/// closed. With `BMCP_MENU_AUTORUN=<command id>` in the environment that entry is chosen
/// instead — how the menu commands are tested live without desktop input.
pub(crate) fn run_offscreen(model: &MenuModel, callback: &RunContextMenuCallback) {
    let labels: Vec<String> = (0..model.count())
        .map(|i| {
            let label = CefString::from(&model.label_at(i)).to_string();
            format!("{}={label}", model.command_id_at(i))
        })
        .collect();
    tracing::info!(?labels, "context menu of an off-screen tab");
    let autorun = std::env::var("BMCP_MENU_AUTORUN")
        .ok()
        .and_then(|v| v.parse::<i32>().ok());
    match autorun {
        Some(id) => callback.cont(id, EventFlags::default()),
        None => callback.cancel(),
    }
}

/// Put `text` on the clipboard.
fn copy(text: Option<&str>) {
    if let Some(text) = text
        && !crate::clipboard::set_text(text)
    {
        tracing::warn!("could not copy to the clipboard");
    }
}

/// Open `url` as a tab right after tab `opener`: in the background in the same profile
/// (like a middle click), or in the foreground in `profile` (another workspace).
fn open_next_to(opener: TabId, url: &str, profile: Option<String>) {
    let Some(ctx) = with_ui(|ui| ui.shell.clone()) else {
        return;
    };
    let facts = with_ui(|ui| {
        let t = ui.tabs.get(&opener)?;
        Some((t.profile.clone(), t.run.clone(), t.window))
    });
    let Some((own_profile, run, window)) = facts else {
        return;
    };
    let (key, activate) = match profile {
        Some(p) => match ProfileKey::from_options(Some(&p), false) {
            Ok(key) => (key, true),
            Err(err) => {
                tracing::warn!(%err, "workspace for the link not usable");
                return;
            }
        },
        None => (own_profile, false),
    };
    let mut pending = PendingTab::new(url, key.clone(), run);
    pending.activate = activate;
    crate::profiles::when_ready(
        &key,
        Box::new(move || match window {
            Some(w) => crate::chrome::add_tab_after(&ctx, w, opener, pending),
            None => crate::shell::open_window(pending, &ctx),
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(entries: &[Entry]) -> Vec<i32> {
        entries
            .iter()
            .map(|e| match e {
                Entry::Item { id, .. } | Entry::Submenu { id, .. } => *id,
                Entry::Separator => 0,
            })
            .collect()
    }

    #[test]
    fn plain_page_has_navigation_ai_and_inspect() {
        let menu = build(&MenuFacts {
            can_back: true,
            ..Default::default()
        });
        assert_eq!(
            ids(&menu),
            [BACK, FORWARD, RELOAD, PRINT, 0, ASK_AI, 0, INSPECT]
        );
        assert!(matches!(&menu[0], Entry::Item { enabled: true, .. }));
        assert!(matches!(&menu[1], Entry::Item { enabled: false, .. }));
        assert!(matches!(&menu[5], Entry::Item { label, .. } if label == "Ask AI about this page"));
    }

    #[test]
    fn link_with_workspaces_and_image() {
        let menu = build(&MenuFacts {
            link: Some("https://a.test/x".into()),
            image: Some("https://a.test/i.png".into()),
            workspaces: vec![
                ("work".into(), "Work".into()),
                ("shop".into(), "Shop".into()),
            ],
            ..Default::default()
        });
        assert_eq!(
            ids(&menu),
            [
                OPEN_LINK_NEW_TAB,
                WORKSPACE_MENU,
                COPY_LINK,
                0,
                OPEN_IMAGE_NEW_TAB,
                SAVE_IMAGE,
                COPY_IMAGE_URL,
                0,
                ASK_AI,
                0,
                INSPECT
            ]
        );
        let Entry::Submenu { items, .. } = &menu[1] else {
            panic!("submenu expected")
        };
        assert_eq!(
            items,
            &[
                (WORKSPACE_FIRST, "Work".into()),
                (WORKSPACE_FIRST + 1, "Shop".into())
            ]
        );
        // No other workspace: no submenu.
        let menu = build(&MenuFacts {
            link: Some("https://a.test/".into()),
            ..Default::default()
        });
        assert!(!ids(&menu).contains(&WORKSPACE_MENU));
        assert!(matches!(&menu[3], Entry::Item { label, .. } if label == "Ask AI about this link"));
    }

    #[test]
    fn text_fields_and_selections() {
        let menu = build(&MenuFacts {
            editable: true,
            selection: Some("hello".into()),
            ..Default::default()
        });
        assert_eq!(
            ids(&menu),
            [
                UNDO, REDO, 0, CUT, COPY, PASTE, SELECT_ALL, 0, ASK_AI, 0, INSPECT
            ]
        );
        let long = "a very long selection that will not fit into one menu line at all";
        let menu = build(&MenuFacts {
            selection: Some(long.into()),
            ..Default::default()
        });
        assert_eq!(ids(&menu), [COPY, 0, ASK_AI, 0, INSPECT]);
        let Entry::Item { label, .. } = &menu[2] else {
            panic!()
        };
        assert_eq!(label, "Ask AI about “a very long selection that will…”");
        assert_eq!(
            ask_payload(
                &MenuFacts {
                    selection: Some(long.into()),
                    ..Default::default()
                },
                "https://p.test/"
            ),
            ("selection", long.to_string())
        );
        assert_eq!(
            ask_payload(&MenuFacts::default(), "https://p.test/"),
            ("page", "https://p.test/".to_string())
        );
    }

    #[test]
    fn builtin_ids_match_cef() {
        assert_eq!(BACK, MenuId::BACK.get_raw());
        assert_eq!(FORWARD, MenuId::FORWARD.get_raw());
        assert_eq!(RELOAD, MenuId::RELOAD.get_raw());
        assert_eq!(UNDO, MenuId::UNDO.get_raw());
        assert_eq!(REDO, MenuId::REDO.get_raw());
        assert_eq!(CUT, MenuId::CUT.get_raw());
        assert_eq!(COPY, MenuId::COPY.get_raw());
        assert_eq!(PASTE, MenuId::PASTE.get_raw());
        assert_eq!(SELECT_ALL, MenuId::SELECT_ALL.get_raw());
        assert_eq!(PRINT, MenuId::PRINT.get_raw());
        assert_eq!(USER_FIRST, MenuId::USER_FIRST.get_raw());
    }
}
