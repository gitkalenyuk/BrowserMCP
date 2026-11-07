//! The context menu of web pages and of our own UI: CEF's native menu (it is drawn over the
//! page by the OS, so it is never hidden under a view) filled with our own model.
//!
//! Three surfaces ([`Surface`]): a web page gets the full menu; one of our pages in a tab
//! (new tab, settings) gets editing, link and Back / Forward / Reload entries; our chrome and
//! the palette get only Cut / Copy / Paste / Select all and "Copy link address" (and no menu
//! at all on bare chrome, where Alloy would offer Back / Reload / View source).
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

/// Where the menu was opened.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum Surface {
    /// A web page in a tab: the full menu.
    #[default]
    Web,
    /// One of our own pages shown in a tab (new tab page, settings, library): editing and
    /// link entries plus Back / Forward / Reload, no "Ask AI" / Print / Inspect.
    OurPage,
    /// Our chrome itself (tab strip, toolbar, side panels) or the Ctrl+K palette: only
    /// Cut / Copy / Paste / Select all and "Copy link address"; nothing on bare chrome.
    OurView,
}

// `cef_context_menu_edit_state_flags_t`: what the focused text field can do right now.
pub(crate) const CAN_UNDO: u32 = 1;
pub(crate) const CAN_REDO: u32 = 2;
pub(crate) const CAN_CUT: u32 = 4;
pub(crate) const CAN_COPY: u32 = 8;
pub(crate) const CAN_PASTE: u32 = 16;
pub(crate) const CAN_SELECT_ALL: u32 = 64;

/// What was right-clicked, as far as the menu cares.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct MenuFacts {
    pub surface: Surface,
    /// `CAN_*` bits of a text field (Undo … Select all are greyed out when they would do
    /// nothing); `None` = unknown, everything enabled.
    pub edit_flags: Option<u32>,
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

/// Undo … Select all for a text field, greyed out by the field's edit flags.
fn edit_group(facts: &MenuFacts) -> Vec<Entry> {
    let can = |bit: u32| facts.edit_flags.is_none_or(|f| f & bit != 0);
    let entry = |id: i32, label: &str, bit: u32| Entry::Item {
        id,
        label: label.to_string(),
        enabled: can(bit),
    };
    vec![
        entry(UNDO, "Undo", CAN_UNDO),
        entry(REDO, "Redo", CAN_REDO),
        Entry::Separator,
        entry(CUT, "Cut", CAN_CUT),
        entry(COPY, "Copy", CAN_COPY),
        entry(PASTE, "Paste", CAN_PASTE),
        entry(SELECT_ALL, "Select all", CAN_SELECT_ALL),
    ]
}

/// "Open link in new tab" (+ other workspaces) and "Copy link address".
fn link_group(facts: &MenuFacts) -> Vec<Entry> {
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
    g
}

fn navigation_group(facts: &MenuFacts) -> Vec<Entry> {
    vec![
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
    ]
}

/// Join groups with separators; there is never a separator at the start, at the end or twice
/// in a row.
fn join(groups: Vec<Vec<Entry>>) -> Vec<Entry> {
    let mut out = Vec::new();
    for g in groups.into_iter().filter(|g| !g.is_empty()) {
        if !out.is_empty() {
            out.push(Entry::Separator);
        }
        out.extend(g);
    }
    out
}

/// The menu for `facts` (pure). An empty menu is not shown at all.
pub(crate) fn build(facts: &MenuFacts) -> Vec<Entry> {
    match facts.surface {
        Surface::Web => build_web(facts),
        Surface::OurPage | Surface::OurView => build_ours(facts),
    }
}

/// Our own pages and views: copy & paste where there is text, links can be copied (and, in
/// a tab, opened), nothing that treats our UI as a web page.
fn build_ours(facts: &MenuFacts) -> Vec<Entry> {
    let in_tab = facts.surface == Surface::OurPage;
    let mut groups: Vec<Vec<Entry>> = Vec::new();
    if facts.link.is_some() {
        groups.push(if in_tab {
            link_group(facts)
        } else {
            vec![item(COPY_LINK, "Copy link address")]
        });
    }
    if facts.editable {
        groups.push(edit_group(facts));
    } else if facts.selection.is_some() {
        groups.push(vec![item(COPY, "Copy"), item(SELECT_ALL, "Select all")]);
    }
    let nothing_special = facts.link.is_none() && !facts.editable && facts.selection.is_none();
    if in_tab && nothing_special {
        groups.push(navigation_group(facts));
    }
    join(groups)
}

/// A web page's menu.
fn build_web(facts: &MenuFacts) -> Vec<Entry> {
    let mut groups: Vec<Vec<Entry>> = Vec::new();
    if facts.link.is_some() {
        groups.push(link_group(facts));
    }
    if facts.image.is_some() {
        groups.push(vec![
            item(OPEN_IMAGE_NEW_TAB, "Open image in new tab"),
            item(SAVE_IMAGE, "Save image (to Downloads)"),
            item(COPY_IMAGE_URL, "Copy image address"),
        ]);
    }
    if facts.editable {
        groups.push(edit_group(facts));
    } else if facts.selection.is_some() {
        groups.push(vec![item(COPY, "Copy")]);
    }
    let nothing_special = facts.link.is_none() && facts.image.is_none() && !facts.editable;
    if nothing_special && facts.selection.is_none() {
        let mut g = navigation_group(facts);
        g.push(item(PRINT, "Print…"));
        groups.push(g);
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
    join(groups)
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

/// Read what was clicked from CEF's params (`in_tab`: see [`fill`]).
pub(crate) fn facts(browser: &Browser, params: &ContextMenuParams, in_tab: bool) -> MenuFacts {
    let non_empty = |s: String| (!s.is_empty()).then_some(s);
    let link = non_empty(text(params.link_url()));
    let image = (params.media_type() == ContextMenuMediaType::IMAGE)
        .then(|| text(params.source_url()))
        .and_then(non_empty);
    let selection = non_empty(text(params.selection_text())).filter(|s| !s.trim().is_empty());
    let editable = params.is_editable() == 1;
    let edit_flags = editable.then(|| {
        let raw: sys::cef_context_menu_edit_state_flags_t = params.edit_state_flags().into();
        raw.0 as u32
    });
    let surface = if !in_tab {
        Surface::OurView
    } else if crate::scheme::is_chrome_url(&text(params.page_url())) {
        Surface::OurPage
    } else {
        Surface::Web
    };
    if surface == Surface::OurView {
        // The chrome is not a tab: no history, no workspaces to open links in.
        return MenuFacts {
            surface,
            edit_flags,
            link,
            image,
            selection,
            editable,
            ..Default::default()
        };
    }
    let id = TabId(browser.identifier());
    let (can_back, can_forward, profile) = with_ui(|ui| {
        ui.tabs.get(&id).map_or((false, false, String::new()), |t| {
            (t.can_go_back, t.can_go_forward, t.profile.name())
        })
    });
    MenuFacts {
        surface,
        edit_flags,
        link,
        image,
        selection,
        editable,
        can_back,
        can_forward,
        workspaces: crate::profiles::workspaces()
            .into_iter()
            .filter(|(id, _)| *id != profile)
            .collect(),
    }
}

/// `on_before_context_menu`: replace CEF's model with ours. `in_tab` = a tab (a web page or
/// one of our pages in it); false = our chrome / palette view.
pub(crate) fn fill(browser: &Browser, params: &ContextMenuParams, model: &MenuModel, in_tab: bool) {
    let facts = facts(browser, params, in_tab);
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

    /// Entries a menu of our own UI must never have.
    const WEB_ONLY: [i32; 6] = [
        ASK_AI,
        INSPECT,
        PRINT,
        SAVE_IMAGE,
        OPEN_IMAGE_NEW_TAB,
        COPY_IMAGE_URL,
    ];

    fn view(facts: MenuFacts) -> Vec<Entry> {
        build(&MenuFacts {
            surface: Surface::OurView,
            ..facts
        })
    }

    fn page(facts: MenuFacts) -> Vec<Entry> {
        build(&MenuFacts {
            surface: Surface::OurPage,
            ..facts
        })
    }

    fn enabled(entries: &[Entry], wanted: i32) -> bool {
        entries
            .iter()
            .any(|e| matches!(e, Entry::Item { id, enabled: true, .. } if *id == wanted))
    }

    #[test]
    fn our_chrome_gets_copy_and_paste_only() {
        // A text field (omnibox, AI chat input, settings field).
        let field = view(MenuFacts {
            editable: true,
            ..Default::default()
        });
        assert_eq!(ids(&field), [UNDO, REDO, 0, CUT, COPY, PASTE, SELECT_ALL]);
        // Selected text in a chat message / log / details.
        let text = view(MenuFacts {
            selection: Some("output line".into()),
            ..Default::default()
        });
        assert_eq!(ids(&text), [COPY, SELECT_ALL]);
        // A link: copy it (the chrome is not a tab to open it next to).
        let link = view(MenuFacts {
            link: Some("https://docs.test/".into()),
            selection: Some("docs".into()),
            ..Default::default()
        });
        assert_eq!(ids(&link), [COPY_LINK, 0, COPY, SELECT_ALL]);
        // Bare chrome (buttons, tab strip): no menu at all.
        assert!(view(MenuFacts::default()).is_empty());
        // Images in the chrome are icons, not content.
        assert!(
            view(MenuFacts {
                image: Some("bmcp://ui/icon.svg".into()),
                ..Default::default()
            })
            .is_empty()
        );
        for menu in [&field, &text, &link] {
            assert!(
                !ids(menu)
                    .iter()
                    .any(|id| WEB_ONLY.contains(id) || *id == BACK)
            );
        }
    }

    #[test]
    fn edit_flags_grey_out_what_the_field_cannot_do() {
        let menu = view(MenuFacts {
            editable: true,
            edit_flags: Some(CAN_PASTE | CAN_SELECT_ALL),
            ..Default::default()
        });
        assert!(enabled(&menu, PASTE));
        assert!(enabled(&menu, SELECT_ALL));
        for id in [UNDO, REDO, CUT, COPY] {
            assert!(!enabled(&menu, id), "{id} should be disabled");
        }
        // With a selection in the field everything that applies is on.
        let menu = view(MenuFacts {
            editable: true,
            edit_flags: Some(CAN_UNDO | CAN_CUT | CAN_COPY | CAN_PASTE | CAN_SELECT_ALL),
            ..Default::default()
        });
        assert!(enabled(&menu, CUT) && enabled(&menu, COPY) && enabled(&menu, UNDO));
        assert!(!enabled(&menu, REDO));
        // Web pages use the same flags.
        let web = build(&MenuFacts {
            editable: true,
            edit_flags: Some(0),
            ..Default::default()
        });
        assert!(!enabled(&web, PASTE));
        assert!(enabled(&web, ASK_AI));
    }

    #[test]
    fn our_pages_in_a_tab_keep_navigation_but_not_web_entries() {
        let plain = page(MenuFacts {
            can_back: true,
            ..Default::default()
        });
        assert_eq!(ids(&plain), [BACK, FORWARD, RELOAD]);
        assert!(enabled(&plain, BACK) && !enabled(&plain, FORWARD));
        // A top site / bookmark tile on the new tab page.
        let link = page(MenuFacts {
            link: Some("https://a.test/".into()),
            workspaces: vec![("work".into(), "Work".into())],
            ..Default::default()
        });
        assert_eq!(ids(&link), [OPEN_LINK_NEW_TAB, WORKSPACE_MENU, COPY_LINK]);
        let field = page(MenuFacts {
            editable: true,
            ..Default::default()
        });
        assert_eq!(ids(&field), [UNDO, REDO, 0, CUT, COPY, PASTE, SELECT_ALL]);
        let text = page(MenuFacts {
            selection: Some("guide text".into()),
            ..Default::default()
        });
        assert_eq!(ids(&text), [COPY, SELECT_ALL]);
        for menu in [&plain, &link, &field, &text] {
            assert!(!ids(menu).iter().any(|id| WEB_ONLY.contains(id)));
        }
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
        type F = sys::cef_context_menu_edit_state_flags_t;
        assert_eq!(CAN_UNDO, F::CM_EDITFLAG_CAN_UNDO.0 as u32);
        assert_eq!(CAN_REDO, F::CM_EDITFLAG_CAN_REDO.0 as u32);
        assert_eq!(CAN_CUT, F::CM_EDITFLAG_CAN_CUT.0 as u32);
        assert_eq!(CAN_COPY, F::CM_EDITFLAG_CAN_COPY.0 as u32);
        assert_eq!(CAN_PASTE, F::CM_EDITFLAG_CAN_PASTE.0 as u32);
        assert_eq!(CAN_SELECT_ALL, F::CM_EDITFLAG_CAN_SELECT_ALL.0 as u32);
    }
}
