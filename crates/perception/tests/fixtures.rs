//! Golden and behaviour tests on recorded-style CDP fixtures in `tests/fixtures/cdp/`
//! (repo root). Golden outputs live in `tests/snapshots/` and are checked with `insta`;
//! after an intended format change run `cargo insta review` (or set
//! `INSTA_UPDATE=always`) and commit the updated `.snap` files.

use std::collections::HashMap;

use bmcp_perception::{
    AxTree, Filter, Mode, NodeLayout, PageHeader, RefRegistry, Scroll, SnapshotOptions, Viewport,
    diff, find, resolve, resolve_group, snapshot,
};

fn load(name: &str) -> AxTree {
    let path = format!(
        "{}/../../tests/fixtures/cdp/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    AxTree::from_json_str(&text).unwrap()
}

fn header(title: &str, url: &str) -> PageHeader {
    PageHeader {
        tab: 1,
        title: title.to_string(),
        url: url.to_string(),
        viewport: None,
        scroll: None,
    }
}

fn login_header() -> PageHeader {
    PageHeader {
        tab: 3,
        title: "Sign in – Acme".to_string(),
        url: "https://acme.com/login".to_string(),
        viewport: Some(Viewport {
            width: 1280,
            height: 720,
        }),
        scroll: Some(Scroll { y: 0, height: 1840 }),
    }
}

fn with_mode(mode: Mode) -> SnapshotOptions {
    SnapshotOptions {
        mode,
        ..SnapshotOptions::default()
    }
}

fn with_root(root: &str) -> SnapshotOptions {
    SnapshotOptions {
        root: Some(root.to_string()),
        ..SnapshotOptions::default()
    }
}

/// Backend id of the first node with this role and name.
fn backend_of(tree: &AxTree, role: &str, name: &str) -> i64 {
    tree.nodes
        .iter()
        .find(|n| n.role() == role && n.name() == name)
        .and_then(|n| n.backend_dom_node_id)
        .unwrap_or_else(|| panic!("no {role} {name:?} in fixture"))
}

// ---------------------------------------------------------------------------------
// Format
// ---------------------------------------------------------------------------------

#[test]
fn example_com_interactive() {
    let tree = load("example_com.json");
    let mut registry = RefRegistry::new();
    let snap = snapshot(
        &tree,
        &mut registry,
        &SnapshotOptions::default(),
        header("Example Domain", "https://example.com/"),
    )
    .unwrap();
    assert_eq!(
        snap.text,
        "snap 1 tab=1 \"Example Domain\" https://example.com/\n\
         - heading \"Example Domain\" h1\n\
         - link \"More information...\" @1:e1"
    );
}

#[test]
fn example_com_full() {
    let tree = load("example_com.json");
    let mut registry = RefRegistry::new();
    let snap = snapshot(
        &tree,
        &mut registry,
        &with_mode(Mode::Full),
        header("Example Domain", "https://example.com/"),
    )
    .unwrap();
    insta::assert_snapshot!(snap.text);
}

#[test]
fn login_interactive() {
    let tree = load("login_form.json");
    let mut registry = RefRegistry::new();
    let snap = snapshot(
        &tree,
        &mut registry,
        &SnapshotOptions::default(),
        login_header(),
    )
    .unwrap();
    let expected = r#"snap 1 tab=3 "Sign in – Acme" https://acme.com/login vp=1280x720 y=0/1840
- banner
  - link "Home" @1:e1
- main
  - heading "Sign in" h1
  - textbox "Email" @1:e2 req focused
  - textbox "Password" @1:e3 pwd
  - checkbox "Remember me" @1:e4 off
  - button "Sign in" @1:e5 disabled
  - list "Providers" 12 items, 3 shown → @1:g1
    - button "Google" @1:e6
    - button "GitHub" @1:e7
    - button "Microsoft" @1:e8
- footer
  - link "Privacy" @1:e9
  - link "Terms" @1:e10
- dialog "Cookies"
  - button "Accept all" @1:e11
  - button "Reject all" @1:e12"#;
    assert_eq!(snap.text, expected);
    assert_eq!(snap.refs.len(), 12);
    assert_eq!(snap.refs[2].ref_str, "@1:e3");
    assert_eq!(snap.refs[2].role, "textbox");
    assert_eq!(snap.refs[2].name, "Password");
    assert!(!snap.skeleton_fallback);
}

#[test]
fn login_full() {
    let tree = load("login_form.json");
    let mut registry = RefRegistry::new();
    let snap = snapshot(&tree, &mut registry, &with_mode(Mode::Full), login_header()).unwrap();
    insta::assert_snapshot!(snap.text);
}

#[test]
fn nested_landmarks_interactive() {
    let tree = load("nested_landmarks.json");
    let mut registry = RefRegistry::new();
    let snap = snapshot(
        &tree,
        &mut registry,
        &SnapshotOptions::default(),
        header(
            "Understanding Accessibility Trees – Acme Blog",
            "https://blog.acme.com/ax-trees",
        ),
    )
    .unwrap();
    insta::assert_snapshot!(snap.text);
}

#[test]
fn product_list_interactive() {
    let tree = load("product_list.json");
    let mut registry = RefRegistry::new();
    let snap = snapshot(
        &tree,
        &mut registry,
        &SnapshotOptions::default(),
        header("Shop – Acme", "https://shop.acme.com/products"),
    )
    .unwrap();
    insta::assert_snapshot!(snap.text);
    assert!(
        snap.text
            .contains("list \"Products\" 40 items, 3 shown → @1:g1")
    );
}

#[test]
fn password_page_interactive() {
    let tree = load("password_checkbox.json");
    let pin = backend_of(&tree, "textbox", "PIN");
    let layout = HashMap::from([(
        pin,
        NodeLayout {
            input_type: Some("password".to_string()),
            ..NodeLayout::default()
        },
    )]);
    let tree = tree.with_layout(layout);
    let mut registry = RefRegistry::new();
    let snap = snapshot(
        &tree,
        &mut registry,
        &SnapshotOptions::default(),
        header(
            "Security settings – Acme",
            "https://acme.com/settings/security",
        ),
    )
    .unwrap();
    insta::assert_snapshot!(snap.text);
}

// ---------------------------------------------------------------------------------
// Refs
// ---------------------------------------------------------------------------------

#[test]
fn refs_are_stable_across_snapshots_of_one_document() {
    let tree = load("login_form.json");
    let mut registry = RefRegistry::new();
    let first = snapshot(
        &tree,
        &mut registry,
        &SnapshotOptions::default(),
        login_header(),
    )
    .unwrap();
    let second = snapshot(
        &tree,
        &mut registry,
        &SnapshotOptions::default(),
        login_header(),
    )
    .unwrap();
    assert_eq!(first.id, 1);
    assert_eq!(second.id, 2);
    // Same text apart from the snapshot number.
    assert_eq!(
        first
            .text
            .replace("@1:", "@2:")
            .replacen("snap 1", "snap 2", 1),
        second.text
    );
    for (a, b) in first.refs.iter().zip(&second.refs) {
        assert_eq!(a.backend_node_id, b.backend_node_id);
        assert_eq!(a.ref_str.replace("@1:", "@2:"), b.ref_str);
    }
    // Old and new prefixes and the bare ref all resolve to the same node.
    let password = backend_of(&tree, "textbox", "Password");
    assert_eq!(resolve(&registry, "@1:e3"), Ok(password));
    assert_eq!(resolve(&registry, "@2:e3"), Ok(password));
    assert_eq!(resolve(&registry, "e3"), Ok(password));

    // A node that only appears later gets the next free number; old numbers stay.
    let after = load("login_form_after_submit.json");
    let third = snapshot(
        &after,
        &mut registry,
        &SnapshotOptions::default(),
        login_header(),
    )
    .unwrap();
    assert!(third.text.contains("button \"Sign in\" @3:e5"));
    assert_eq!(resolve(&registry, "e3"), Ok(password));
}

#[test]
fn new_document_resets_refs() {
    let tree = load("login_form.json");
    let mut registry = RefRegistry::new();
    snapshot(
        &tree,
        &mut registry,
        &SnapshotOptions::default(),
        login_header(),
    )
    .unwrap();
    assert!(resolve(&registry, "e3").is_ok());

    registry.new_document();
    // Before any new snapshot, everything is stale.
    assert_eq!(resolve(&registry, "e3").unwrap_err().code(), "STALE_REF");
    assert_eq!(resolve(&registry, "@1:e3").unwrap_err().code(), "STALE_REF");
    assert_eq!(
        resolve_group(&registry, "@1:g1").unwrap_err().code(),
        "STALE_REF"
    );

    // The new document numbers its refs after the old ones, so old refs never alias.
    let example = load("example_com.json");
    let snap = snapshot(
        &example,
        &mut registry,
        &SnapshotOptions::default(),
        header("Example Domain", "https://example.com/"),
    )
    .unwrap();
    assert_eq!(snap.id, 2);
    assert_eq!(snap.refs[0].ref_str, "@2:e13");
    assert_eq!(resolve(&registry, "e13"), Ok(snap.refs[0].backend_node_id));
    assert_eq!(resolve(&registry, "e3").unwrap_err().code(), "STALE_REF");
    assert_eq!(resolve(&registry, "@1:e3").unwrap_err().code(), "STALE_REF");
}

#[test]
fn resolve_good_stale_and_bad_refs() {
    let tree = load("login_form.json");
    let mut registry = RefRegistry::new();
    assert_eq!(resolve(&registry, "e1").unwrap_err().code(), "BAD_REF"); // no snapshot yet
    snapshot(
        &tree,
        &mut registry,
        &SnapshotOptions::default(),
        login_header(),
    )
    .unwrap();

    assert_eq!(
        resolve(&registry, "@1:e1"),
        Ok(backend_of(&tree, "link", "Home"))
    );
    assert_eq!(
        resolve(&registry, " @e1 "),
        Ok(backend_of(&tree, "link", "Home"))
    );
    for bad in ["@1:e99", "@5:e1", "hello", "e", "@1:x1", "@1:g1", ""] {
        let err = resolve(&registry, bad).unwrap_err();
        assert_eq!(err.code(), "BAD_REF", "{bad}: {err}");
    }
    let group = resolve_group(&registry, "@1:g1").unwrap();
    assert_eq!(group.role, "list");
    assert_eq!(group.name, "Providers");
    assert_eq!(
        resolve_group(&registry, "@1:g7").unwrap_err().code(),
        "BAD_REF"
    );
    assert_eq!(
        resolve_group(&registry, "@1:e1").unwrap_err().code(),
        "BAD_REF"
    );
}

// ---------------------------------------------------------------------------------
// Skeleton and drill-down
// ---------------------------------------------------------------------------------

#[test]
fn skeleton_then_drill_down_by_group_ref() {
    let tree = load("nested_landmarks.json");
    let mut registry = RefRegistry::new();
    let page = header(
        "Understanding Accessibility Trees – Acme Blog",
        "https://blog.acme.com/ax-trees",
    );
    let skeleton = snapshot(
        &tree,
        &mut registry,
        &with_mode(Mode::Skeleton { depth: 1 }),
        page.clone(),
    )
    .unwrap();
    insta::assert_snapshot!("nested_skeleton_depth1", skeleton.text);
    assert!(skeleton.refs.is_empty());

    // Find the group ref of `main` and drill into it.
    let main_line = skeleton
        .text
        .lines()
        .find(|l| l.starts_with("- main"))
        .unwrap();
    let group = main_line.rsplit(' ').next().unwrap();
    assert_eq!(group, "@1:g2");
    let drill = snapshot(&tree, &mut registry, &with_root(group), page.clone()).unwrap();
    insta::assert_snapshot!("nested_drill_main", drill.text);
    assert!(drill.text.starts_with("snap 2 tab=1"));
    assert!(drill.text.contains(" root=@1:g2"));
    assert!(drill.text.contains("button \"Post comment\" @2:e"));
    assert!(!drill.text.contains("footer"));

    // Depth 2 shows the landmarks' children and collapses below that.
    let depth2 = snapshot(
        &tree,
        &mut registry,
        &with_mode(Mode::Skeleton { depth: 2 }),
        page,
    )
    .unwrap();
    insta::assert_snapshot!("nested_skeleton_depth2", depth2.text);
}

#[test]
fn drill_down_into_collapsed_list_shows_every_item() {
    let tree = load("product_list.json");
    let mut registry = RefRegistry::new();
    let page = header("Shop – Acme", "https://shop.acme.com/products");
    snapshot(
        &tree,
        &mut registry,
        &SnapshotOptions::default(),
        page.clone(),
    )
    .unwrap();
    let drill = snapshot(&tree, &mut registry, &with_root("g1"), page).unwrap();
    assert!(drill.text.starts_with(
        "snap 2 tab=1 \"Shop – Acme\" https://shop.acme.com/products root=g1\n- list \"Products\"\n"
    ));
    assert_eq!(drill.text.matches("button \"Add to cart\"").count(), 40);
    assert!(drill.text.contains("link \"Product 40\""));
    assert!(!drill.skeleton_fallback);
}

#[test]
fn drill_down_by_element_ref() {
    let tree = load("nested_landmarks.json");
    let mut registry = RefRegistry::new();
    let page = header("Blog", "https://blog.acme.com/ax-trees");
    let snap = snapshot(
        &tree,
        &mut registry,
        &SnapshotOptions::default(),
        page.clone(),
    )
    .unwrap();
    let combo = snap.refs.iter().find(|r| r.role == "combobox").unwrap();
    let drill = snapshot(&tree, &mut registry, &with_root(&combo.ref_str), page).unwrap();
    insta::assert_snapshot!("nested_drill_combobox", drill.text);
    // The same e-number is used for the root in the new snapshot.
    let number = combo.ref_str.split(':').nth(1).unwrap();
    assert!(
        drill
            .text
            .contains(&format!("combobox \"Sort by\" @2:{number}"))
    );
}

#[test]
fn drill_down_errors() {
    let tree = load("nested_landmarks.json");
    let mut registry = RefRegistry::new();
    let page = header("Blog", "https://blog.acme.com/ax-trees");
    snapshot(
        &tree,
        &mut registry,
        &SnapshotOptions::default(),
        page.clone(),
    )
    .unwrap();
    let err = snapshot(&tree, &mut registry, &with_root("@1:g9"), page.clone()).unwrap_err();
    assert_eq!(err.code(), "BAD_REF");
    // A ref whose node is gone from the tree we are given.
    let other = load("example_com.json");
    let err = snapshot(&other, &mut registry, &with_root("e3"), page).unwrap_err();
    assert_eq!(err.code(), "NOT_FOUND");
}

// ---------------------------------------------------------------------------------
// Budget
// ---------------------------------------------------------------------------------

#[test]
fn over_budget_falls_back_to_skeleton() {
    let tree = load("product_list.json");
    let mut registry = RefRegistry::new();
    let page = header("Shop – Acme", "https://shop.acme.com/products");
    let options = SnapshotOptions {
        collapse_lists_over: 1000, // no list collapsing: force a big text
        max_chars: 2000,
        ..SnapshotOptions::default()
    };
    let snap = snapshot(&tree, &mut registry, &options, page.clone()).unwrap();
    insta::assert_snapshot!("product_list_over_budget", snap.text);
    assert!(snap.skeleton_fallback);
    assert!(!snap.cut);
    assert!(snap.text.chars().count() <= 2000);
    let notice = snap.text.lines().nth(1).unwrap();
    assert!(notice.starts_with("! "), "{notice}");
    assert!(notice.contains("over the 2000 budget"));

    // The groups in the fallback text really work as drill-down roots.
    let main_group = snap
        .text
        .lines()
        .find(|l| l.contains("list \"Products\""))
        .and_then(|l| l.rsplit(' ').next())
        .unwrap()
        .to_string();
    assert!(resolve_group(&registry, &main_group).is_ok());

    // A tiny budget cuts lines but stays within budget and says so.
    let tiny = SnapshotOptions {
        max_chars: 200,
        ..options
    };
    let snap = snapshot(&tree, &mut registry, &tiny, page).unwrap();
    assert!(snap.cut);
    assert!(snap.text.chars().count() <= 200, "{}", snap.text);
    assert!(
        snap.text
            .lines()
            .last()
            .unwrap()
            .starts_with("… cut to fit 200 chars")
    );
}

// ---------------------------------------------------------------------------------
// Find
// ---------------------------------------------------------------------------------

#[test]
fn find_by_role_and_name() {
    let tree = load("login_form.json");
    let mut registry = RefRegistry::new();
    let filter = Filter {
        role: Some("button".to_string()),
        name: Some("sign IN".to_string()),
        ..Filter::default()
    };
    let result = find(&tree, &mut registry, &filter, 10).unwrap();
    assert_eq!(result.total, 1);
    assert_eq!(
        result.to_text(),
        "find 1: 1 of 1 matches\n- button \"Sign in\" @1:e1 disabled in main"
    );
    // The ref from find is directly actionable.
    assert_eq!(
        resolve(&registry, "@1:e1"),
        Ok(backend_of(&tree, "button", "Sign in"))
    );

    // Aliased roles work both ways.
    let footer = Filter {
        role: Some("footer".to_string()),
        ..Filter::default()
    };
    assert_eq!(find(&tree, &mut registry, &footer, 10).unwrap().total, 1);
}

#[test]
fn find_text_returns_all_matches_with_context() {
    let tree = load("product_list.json");
    let mut registry = RefRegistry::new();
    let filter = Filter {
        text: Some("add to CART".to_string()),
        ..Filter::default()
    };
    let result = find(&tree, &mut registry, &filter, 3).unwrap();
    assert_eq!(result.total, 40);
    assert_eq!(result.matches.len(), 3);
    insta::assert_snapshot!("find_add_to_cart", result.to_text());
    // Contexts tell the identical buttons apart.
    assert_ne!(result.matches[0].context, result.matches[1].context);
}

#[test]
fn find_regex_and_plain_text() {
    let tree = load("login_form_after_submit.json");
    let mut registry = RefRegistry::new();
    let regex = Filter {
        regex: Some("^Git(Hub|Lab)$".to_string()),
        ..Filter::default()
    };
    let result = find(&tree, &mut registry, &regex, 10).unwrap();
    let names: Vec<&str> = result.matches.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(names, ["GitHub", "GitLab"]);

    // Plain text (an error message inside an unnamed alert) is found too.
    let text = Filter {
        text: Some("wrong password".to_string()),
        ..Filter::default()
    };
    let result = find(&tree, &mut registry, &text, 10).unwrap();
    assert_eq!(result.total, 1);
    assert_eq!(result.matches[0].role, "text");
    assert_eq!(result.matches[0].context, "main");
}

#[test]
fn find_rejects_bad_filters() {
    let tree = load("login_form.json");
    let mut registry = RefRegistry::new();
    let empty = find(&tree, &mut registry, &Filter::default(), 10).unwrap_err();
    assert_eq!(empty.code(), "BAD_FILTER");
    let bad_regex = Filter {
        regex: Some("(".to_string()),
        ..Filter::default()
    };
    assert_eq!(
        find(&tree, &mut registry, &bad_regex, 10)
            .unwrap_err()
            .code(),
        "BAD_FILTER"
    );
}

// ---------------------------------------------------------------------------------
// Diff
// ---------------------------------------------------------------------------------

#[test]
fn diff_after_failed_login() {
    let before = load("login_form.json");
    let after = load("login_form_after_submit.json");
    let mut registry = RefRegistry::new();
    let first = snapshot(
        &before,
        &mut registry,
        &SnapshotOptions::default(),
        login_header(),
    )
    .unwrap();
    let second = snapshot(
        &after,
        &mut registry,
        &SnapshotOptions::default(),
        login_header(),
    )
    .unwrap();
    assert_eq!(
        diff(&first, &second),
        "diff 1→2: + alert \"Wrong password\" \
         | ~ @2:e2 textbox \"Email\" =\"jane@acme.com\" blurred \
         | ~ @2:e3 textbox \"Password\" invalid focused \
         | ~ @2:e5 button \"Sign in\" enabled \
         | - dialog \"Cookies\""
    );
    assert_eq!(diff(&second, &second), "diff 2→2: no changes");
}

// ---------------------------------------------------------------------------------
// Passwords
// ---------------------------------------------------------------------------------

#[test]
fn password_values_are_never_printed_or_searched() {
    let tree = load("password_checkbox.json");
    let pin = backend_of(&tree, "textbox", "PIN");
    let layout = HashMap::from([(
        pin,
        NodeLayout {
            input_type: Some("password".to_string()),
            ..NodeLayout::default()
        },
    )]);
    let tree = tree.with_layout(layout);
    let mut registry = RefRegistry::new();
    let page = header(
        "Security settings – Acme",
        "https://acme.com/settings/security",
    );
    for mode in [Mode::Interactive, Mode::Full, Mode::Skeleton { depth: 3 }] {
        let snap = snapshot(&tree, &mut registry, &with_mode(mode), page.clone()).unwrap();
        for secret in ["hunter2", "4321", "•"] {
            assert!(
                !snap.text.contains(secret),
                "{mode:?} leaked {secret}:\n{}",
                snap.text
            );
        }
        if mode != (Mode::Skeleton { depth: 3 }) {
            assert_eq!(snap.text.matches(" pwd").count(), 3, "{}", snap.text);
        }
    }
    for secret in ["hunter2", "4321", "•"] {
        let filter = Filter {
            text: Some(secret.to_string()),
            ..Filter::default()
        };
        let result = find(&tree, &mut registry, &filter, 10).unwrap();
        assert_eq!(result.total, 0, "find leaked {secret}");
    }
}

#[test]
fn checkbox_switch_and_radio_states() {
    let tree = load("password_checkbox.json");
    let mut registry = RefRegistry::new();
    let snap = snapshot(
        &tree,
        &mut registry,
        &SnapshotOptions::default(),
        header("Security", "https://acme.com/settings/security"),
    )
    .unwrap();
    let line = |name: &str| {
        snap.text
            .lines()
            .find(|l| l.contains(&format!("\"{name}\"")))
            .unwrap()
            .trim()
            .to_string()
    };
    assert!(line("Enable two-factor authentication").ends_with(" on"));
    assert!(line("Email alerts").starts_with("- switch") && line("Email alerts").ends_with(" off"));
    assert!(line("15 minutes").starts_with("- radio") && line("15 minutes").ends_with(" on"));
    assert!(line("Never").ends_with(" off"));
}
