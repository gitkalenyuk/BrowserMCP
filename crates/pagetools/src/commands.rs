//! The registry commands of this crate.

use std::time::Duration;

use bmcp_core::{BackendError, CmdError, Command, Ctx, ErrorCode, Meta, TabId, Toolset};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::dialog::{self, DialogState, OpenDialog, Policy, Rule};
use crate::print::{self, Paper, PdfOptions};
use crate::upload;
use crate::util::{data_dir, resolve_ref};

// ------------------------------------------------------------------ page.dialog

pub struct Dialog;

/// OK or Cancel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DialogAction {
    /// OK / Leave.
    Accept,
    /// Cancel / Stay.
    Dismiss,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DialogArgs {
    /// Tab id from tabs_list. Default: the most recently opened tab.
    pub tab: Option<TabId>,
    pub action: DialogAction,
    /// Text to type into a prompt before accepting it.
    pub text: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct DialogOutput {
    pub tab: TabId,
    pub action: DialogAction,
    /// The dialog that was answered (null when it opened before the browser watched the tab).
    pub dialog: Option<OpenDialog>,
}

impl Command for Dialog {
    const NAME: &'static str = "page.dialog";
    type Args = DialogArgs;
    type Output = DialogOutput;

    fn meta() -> Meta {
        Meta::new(
            "Answer the JavaScript dialog (alert/confirm/prompt/beforeunload) open on a tab: \
             accept (OK; prompts take `text`) or dismiss (Cancel). While a dialog is open, \
             other page_*/input_*/nav_* commands on the tab fail with DIALOG_OPEN, whose \
             candidates[0] describes the dialog. NOT_FOUND when no dialog is open.",
            Toolset::Core,
        )
        .mutates_tab()
        .act()
    }

    fn target_tab(args: &DialogArgs) -> Option<TabId> {
        args.tab
    }

    async fn run(ctx: Ctx, args: DialogArgs) -> Result<DialogOutput, CmdError> {
        let tab = ctx.resolve_tab(args.tab).await?;
        // Dialog events need the Page domain; tabs opened before `start` get it here.
        dialog::ensure_watched_ctx(&ctx, tab).await?;
        let state = DialogState::from_ctx(&ctx);
        let accept = args.action == DialogAction::Accept;
        let mut params = json!({ "accept": accept });
        if let Some(text) = &args.text {
            params["promptText"] = json!(text);
        }
        let reply = ctx
            .backend
            .cdp(tab, "Page.handleJavaScriptDialog".into(), params)
            .await;
        // The dialog is answered (or was not there): it is not open any more either way.
        let dialog = state.take_open(tab);
        match reply {
            Ok(_) => Ok(DialogOutput {
                tab,
                action: args.action,
                dialog,
            }),
            // "No dialog is showing".
            Err(BackendError::Cdp(_)) => Err(CmdError::new(
                ErrorCode::NotFound,
                format!("no JavaScript dialog is open on tab {tab}"),
            )
            .with_retry(bmcp_core::Retry::No)),
            Err(other) => Err(other.into()),
        }
    }
}

// ------------------------------------------------------------------ page.dialog_policy

pub struct DialogPolicy;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DialogPolicyArgs {
    /// Tab id from tabs_list. Default: the most recently opened tab (unless `global`).
    pub tab: Option<TabId>,
    /// Change the rules of every tab (tab rules still win over global ones).
    #[serde(default)]
    pub global: bool,
    /// Rules for each kind: accept | dismiss | ask (leave it to the agent/human) |
    /// auto (accept while automation is active, else ask). Omitted = unchanged.
    pub alert: Option<Rule>,
    pub confirm: Option<Rule>,
    pub prompt: Option<Rule>,
    pub beforeunload: Option<Rule>,
    /// Text for prompts that are accepted automatically (default: their prefilled text).
    pub prompt_text: Option<String>,
    /// Drop this level's rules first (back to the global rules / the defaults).
    #[serde(default)]
    pub reset: bool,
}

/// All four rules, after falling back tab → global → defaults.
#[derive(Debug, Serialize, JsonSchema)]
pub struct EffectivePolicy {
    pub alert: Rule,
    pub confirm: Rule,
    pub prompt: Rule,
    pub beforeunload: Rule,
    pub prompt_text: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct DialogPolicyOutput {
    /// "global" or "tab".
    pub scope: String,
    /// The tab (null for the global scope).
    pub tab: Option<TabId>,
    /// The rules now in force for this scope.
    pub policy: EffectivePolicy,
    /// True when automation counts as active (a non-UI caller ran a command in the last
    /// minute): `auto` rules accept right now.
    pub automation_active: bool,
}

impl Command for DialogPolicy {
    const NAME: &'static str = "page.dialog_policy";
    type Args = DialogPolicyArgs;
    type Output = DialogPolicyOutput;

    fn meta() -> Meta {
        Meta::new(
            "Set how JavaScript dialogs are handled automatically, per tab or `global`: \
             alert/confirm/prompt/beforeunload = accept | dismiss | ask | auto. Default: ask \
             (left open: page_dialog_opened event, DIALOG_OPEN errors), beforeunload = auto \
             (accepted while automation runs). Call without rules to read the policy.",
            Toolset::Core,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: DialogPolicyArgs) -> Result<DialogPolicyOutput, CmdError> {
        if args.global && args.tab.is_some() {
            return Err(CmdError::invalid_params("give either tab or global, not both"));
        }
        let tab = if args.global {
            None
        } else {
            let tab = ctx.resolve_tab(args.tab).await?;
            // Auto-handling needs the tab's dialog events.
            dialog::ensure_watched_ctx(&ctx, tab).await?;
            Some(tab)
        };
        let state = DialogState::from_ctx(&ctx);
        let changes = Policy {
            alert: args.alert,
            confirm: args.confirm,
            prompt: args.prompt,
            beforeunload: args.beforeunload,
            prompt_text: args.prompt_text,
        };
        state.set_policy(tab, &changes, args.reset);
        let effective = state.effective(tab);
        Ok(DialogPolicyOutput {
            scope: if tab.is_some() { "tab" } else { "global" }.to_string(),
            tab,
            policy: EffectivePolicy {
                alert: effective.rule(dialog::DialogType::Alert),
                confirm: effective.rule(dialog::DialogType::Confirm),
                prompt: effective.rule(dialog::DialogType::Prompt),
                beforeunload: effective.rule(dialog::DialogType::Beforeunload),
                prompt_text: effective.prompt_text,
            },
            automation_active: state.automation_active(),
        })
    }
}

// ------------------------------------------------------------------ page.upload

pub struct Upload;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UploadArgs {
    /// Tab id from tabs_list. Default: the most recently opened tab.
    pub tab: Option<TabId>,
    /// The file input, or the button/label that opens the file chooser (from page_snapshot).
    #[serde(rename = "ref")]
    pub target: String,
    /// Files to upload: absolute paths, or paths relative to data/runs/<run_id>/.
    pub files: Vec<String>,
    /// Run whose folder relative paths are resolved in (see run_list).
    pub run_id: Option<String>,
    /// How long to wait for the file chooser after clicking a button (default 5000).
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct UploadOutput {
    pub tab: TabId,
    /// The files as given to the page (absolute paths).
    pub files: Vec<String>,
    /// "input" (the ref was the file input) or "chooser" (clicking it opened a chooser).
    pub via: String,
}

impl Command for Upload {
    const NAME: &'static str = "page.upload";
    type Args = UploadArgs;
    type Output = UploadOutput;

    fn meta() -> Meta {
        Meta::new(
            "Upload files into a page: `ref` is the <input type=file> or the button that \
             opens the file chooser (it is clicked and the chooser filled, no OS dialog). \
             Paths are absolute or relative to data/runs/<run_id>/ and must exist.",
            Toolset::Core,
        )
        .mutates_tab()
        .act()
    }

    fn target_tab(args: &UploadArgs) -> Option<TabId> {
        args.tab
    }

    async fn run(ctx: Ctx, args: UploadArgs) -> Result<UploadOutput, CmdError> {
        let tab = ctx.resolve_tab(args.tab).await?;
        // Check the files before touching the page.
        let paths = upload::resolve_files(
            data_dir(&ctx).root(),
            args.run_id.as_deref(),
            &args.files,
        )?;
        let files: Vec<String> = paths
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        let node = resolve_ref(&ctx, tab, &args.target).await?;
        let timeout = args
            .timeout_ms
            .map_or(upload::DEFAULT_CHOOSER_TIMEOUT, Duration::from_millis);
        let via = upload::upload(&ctx, tab, node, &files, timeout).await?;
        Ok(UploadOutput {
            tab,
            files,
            via: via.as_str().to_string(),
        })
    }
}

// ------------------------------------------------------------------ page.pdf / page.mhtml

pub struct Pdf;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PdfArgs {
    /// Tab id from tabs_list. Default: the most recently opened tab.
    pub tab: Option<TabId>,
    /// Output file: absolute, or relative to data/pdf/. Default: data/pdf/tab<N>-<time>.pdf.
    pub path: Option<String>,
    /// Landscape pages (default false).
    #[serde(default)]
    pub landscape: bool,
    /// Print background colours and images (default false, like the print dialog).
    #[serde(default)]
    pub print_background: bool,
    /// Zoom of the page content, 0.1–2 (default 1).
    pub scale: Option<f64>,
    /// Paper size (default letter).
    #[serde(default)]
    pub paper: Paper,
}

/// A file written by `page.pdf` / `page.mhtml`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct FileOutput {
    pub tab: TabId,
    /// Absolute path of the file.
    pub path: String,
    /// File size in bytes.
    pub bytes: u64,
}

impl Command for Pdf {
    const NAME: &'static str = "page.pdf";
    type Args = PdfArgs;
    type Output = FileOutput;

    fn meta() -> Meta {
        Meta::new(
            "Print the page to a PDF file (data/pdf/ or `path`); returns the path and size.",
            Toolset::Data,
        )
        .palette("Save page as PDF")
    }

    async fn run(ctx: Ctx, args: PdfArgs) -> Result<FileOutput, CmdError> {
        let tab = ctx.resolve_tab(args.tab).await?;
        let options = PdfOptions {
            landscape: args.landscape,
            print_background: args.print_background,
            scale: args.scale.unwrap_or(1.0),
            paper: args.paper,
        };
        let (path, bytes) = print::save_pdf(&ctx, tab, args.path.as_deref(), &options).await?;
        Ok(FileOutput {
            tab,
            path: path.to_string_lossy().into_owned(),
            bytes,
        })
    }
}

pub struct Mhtml;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MhtmlArgs {
    /// Tab id from tabs_list. Default: the most recently opened tab.
    pub tab: Option<TabId>,
    /// Output file: absolute, or relative to data/mhtml/. Default: data/mhtml/tab<N>-<time>.mhtml.
    pub path: Option<String>,
}

impl Command for Mhtml {
    const NAME: &'static str = "page.mhtml";
    type Args = MhtmlArgs;
    type Output = FileOutput;

    fn meta() -> Meta {
        Meta::new(
            "Save the page as one MHTML file (HTML + images + styles; data/mhtml/ or `path`); \
             returns the path and size.",
            Toolset::Data,
        )
    }

    async fn run(ctx: Ctx, args: MhtmlArgs) -> Result<FileOutput, CmdError> {
        let tab = ctx.resolve_tab(args.tab).await?;
        let (path, bytes) = print::save_mhtml(&ctx, tab, args.path.as_deref()).await?;
        Ok(FileOutput {
            tab,
            path: path.to_string_lossy().into_owned(),
            bytes,
        })
    }
}

// ------------------------------------------------------------------ page.find_text / find_clear

pub struct FindText;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FindTextArgs {
    /// Tab id from tabs_list. Default: the most recently opened tab.
    pub tab: Option<TabId>,
    /// Text to find (plain text, not a regex). "" clears the highlights.
    pub query: String,
    /// Match upper/lower case exactly (default false).
    #[serde(default)]
    pub case: bool,
    /// Go to the next match of the same query (wraps around).
    #[serde(default)]
    pub next: bool,
    /// Go to the previous match of the same query (wraps around).
    #[serde(default)]
    pub prev: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct FindTextOutput {
    pub tab: TabId,
    /// Number of visible matches (at most 1000).
    pub count: u32,
    /// 1-based number of the current match (0 when nothing matched).
    pub index: u32,
    /// Text around the current match.
    pub context: String,
}

impl Command for FindText {
    const NAME: &'static str = "page.find_text";
    type Args = FindTextArgs;
    type Output = FindTextOutput;

    fn meta() -> Meta {
        Meta::new(
            "Find text in the page like Ctrl+F: highlight every visible match, scroll to the \
             current one; `next`/`prev` step through them. Returns count, 1-based index and \
             the text around the match.",
            Toolset::Ui,
        )
    }

    async fn run(ctx: Ctx, args: FindTextArgs) -> Result<FindTextOutput, CmdError> {
        if args.next && args.prev {
            return Err(CmdError::invalid_params("give next or prev, not both"));
        }
        let tab = ctx.resolve_tab(args.tab).await?;
        let step = match (args.next, args.prev) {
            (true, _) => 1,
            (_, true) => -1,
            _ => 0,
        };
        let found = crate::find::find_text(&ctx, tab, &args.query, args.case, step).await?;
        if found.unsupported {
            return Err(CmdError::new(
                ErrorCode::NotImplemented,
                "this page has no CSS highlight API (find in page needs it)",
            ));
        }
        Ok(FindTextOutput {
            tab,
            count: found.count,
            index: if found.count == 0 { 0 } else { found.index + 1 },
            context: found.context,
        })
    }
}

pub struct FindClear;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FindClearArgs {
    /// Tab id from tabs_list. Default: the most recently opened tab.
    pub tab: Option<TabId>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct FindClearOutput {
    pub tab: TabId,
}

impl Command for FindClear {
    const NAME: &'static str = "page.find_clear";
    type Args = FindClearArgs;
    type Output = FindClearOutput;

    fn meta() -> Meta {
        Meta::new(
            "Remove the find-in-page highlights (page_find_text) from a tab.",
            Toolset::Ui,
        )
    }

    async fn run(ctx: Ctx, args: FindClearArgs) -> Result<FindClearOutput, CmdError> {
        let tab = ctx.resolve_tab(args.tab).await?;
        crate::find::clear(&ctx, tab).await?;
        Ok(FindClearOutput { tab })
    }
}
