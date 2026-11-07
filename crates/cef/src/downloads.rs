//! Downloads: straight into the download folder, or through the Save as dialog.
//!
//! * a tab of a mass run → `data/runs/<run>/downloads/`, any other tab → the folder chosen in
//!   Settings → Downloads (default `data/downloads/`; `bmcp_settings::downloads`, safe file
//!   names: `bmcp_profiles::downloads`);
//! * "Ask where to save each file" shows Chromium's Save as dialog with that path filled in
//!   (never for mass-run tabs: nobody would answer it);
//! * events on the bus: `download.started`, `download.progress` (at most every 500 ms per
//!   download), `download.done` (complete, cancelled or interrupted). `bmcp_data` records
//!   them in the `downloads` table (`downloads.list`).
//!
//! Both callbacks run on the UI thread.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use bmcp_core::TabId;
use bmcp_profiles::cookies::cef_to_unix_ms;
use bmcp_profiles::downloads::{download_dir, safe_file_name, unique_path};
use bmcp_settings::DownloadsConfig;
use cef::*;
use serde_json::{Value, json};

use crate::bridge::{emit, with_ui};

/// Per download id: where it goes, which tab/run, when we last emitted progress.
struct Tracked {
    path: PathBuf,
    tab: Option<TabId>,
    run: Option<String>,
    last_emit: Instant,
}

#[derive(Default)]
struct State {
    downloads: HashMap<u32, Tracked>,
    /// Paths handed out but not finished yet (two downloads of `a.pdf` at once get
    /// `a.pdf` and `a (1).pdf`).
    reserved: HashSet<PathBuf>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn with_state<T>(f: impl FnOnce(&mut State) -> T) -> T {
    let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
    f(state.get_or_insert_with(State::default))
}

/// The folder for a download of a tab of `run` (if any).
fn target_dir(config: &DownloadsConfig, run: Option<&str>) -> PathBuf {
    let data_root = &crate::profiles::launch().data_root;
    match crate::settings() {
        Some(_) => bmcp_settings::downloads::download_dir(config, data_root, run),
        None => download_dir(data_root, run),
    }
}

pub(crate) fn handler() -> DownloadHandler {
    ShellDownloadHandler::new()
}

fn text(s: CefStringUserfree) -> String {
    CefString::from(&s).to_string()
}

/// The event payload for `item`.
fn payload(item: &DownloadItem, tracked: Option<&Tracked>) -> Value {
    let state = if item.is_complete() == 1 {
        "complete"
    } else if item.is_canceled() == 1 {
        "cancelled"
    } else if item.is_interrupted() == 1 {
        "interrupted"
    } else {
        "in_progress"
    };
    let full_path = text(item.full_path());
    let path = if full_path.is_empty() {
        tracked
            .map(|t| crate::native_path(&t.path))
            .unwrap_or_default()
    } else {
        full_path
    };
    // Chromium's `DownloadInterruptReason` number (0 = none), for diagnosis.
    let interrupt_reason =
        sys::cef_download_interrupt_reason_t::from(item.interrupt_reason()) as i32;
    json!({
        "id": item.id(),
        "url": text(item.url()),
        "path": path,
        "mime": text(item.mime_type()),
        "total_bytes": item.total_bytes(),
        "received_bytes": item.received_bytes(),
        "percent": item.percent_complete(),
        "state": state,
        "interrupt_reason": interrupt_reason,
        "started": cef_to_unix_ms(item.start_time().val),
        "ended": if state == "in_progress" { None } else { cef_to_unix_ms(item.end_time().val) },
        "tab": tracked.and_then(|t| t.tab),
        "run": tracked.and_then(|t| t.run.clone()),
    })
}

wrap_download_handler! {
    struct ShellDownloadHandler;

    impl DownloadHandler {
        fn can_download(
            &self,
            _browser: Option<&mut Browser>,
            _url: Option<&CefString>,
            _request_method: Option<&CefString>,
        ) -> ::std::os::raw::c_int {
            1
        }

        fn on_before_download(
            &self,
            browser: Option<&mut Browser>,
            download_item: Option<&mut DownloadItem>,
            suggested_name: Option<&CefString>,
            callback: Option<&mut BeforeDownloadCallback>,
        ) -> ::std::os::raw::c_int {
            let (Some(item), Some(callback)) = (download_item, callback) else { return 0 };
            let tab = browser.map(|b| TabId(b.identifier()));
            let run = tab.and_then(|id| with_ui(|ui| ui.tabs.get(&id).and_then(|t| t.run.clone())));
            let config = crate::settings().map(|s| s.downloads()).unwrap_or_default();
            let ask = config.ask && run.is_none();
            let dir = target_dir(&config, run.as_deref());
            if let Err(err) = std::fs::create_dir_all(&dir) {
                tracing::warn!(dir = %dir.display(), %err, "download folder not created");
            }
            let name = safe_file_name(&suggested_name.map(CefString::to_string).unwrap_or_default());
            let path = with_state(|s| {
                let path = unique_path(&dir, &name, |p| p.exists() || s.reserved.contains(p));
                s.reserved.insert(path.clone());
                s.downloads.insert(
                    item.id(),
                    Tracked { path: path.clone(), tab, run, last_emit: Instant::now() },
                );
                path
            });
            tracing::info!(path = %path.display(), "download started");
            // The Save as dialog only when the user asked for it in Settings.
            callback.cont(Some(&CefString::from(crate::native_path(&path).as_str())), i32::from(ask));
            let data = with_state(|s| payload(item, s.downloads.get(&item.id())));
            emit(bmcp_data::downloads::DOWNLOAD_STARTED, data);
            1
        }

        fn on_download_updated(
            &self,
            _browser: Option<&mut Browser>,
            download_item: Option<&mut DownloadItem>,
            _callback: Option<&mut DownloadItemCallback>,
        ) {
            let Some(item) = download_item else { return };
            let id = item.id();
            let finished = item.is_complete() == 1 || item.is_canceled() == 1 || item.is_interrupted() == 1;
            let (topic, data) = with_state(|s| {
                let tracked = s.downloads.get_mut(&id)?;
                if finished {
                    let tracked = s.downloads.remove(&id)?;
                    s.reserved.remove(&tracked.path);
                    return Some((bmcp_data::downloads::DOWNLOAD_DONE, payload(item, Some(&tracked))));
                }
                if tracked.last_emit.elapsed() < Duration::from_millis(500) {
                    return None;
                }
                tracked.last_emit = Instant::now();
                Some((bmcp_data::downloads::DOWNLOAD_PROGRESS, payload(item, Some(tracked))))
            })
            .unzip();
            if let (Some(topic), Some(data)) = (topic, data) {
                if topic == bmcp_data::downloads::DOWNLOAD_DONE {
                    tracing::info!(
                        id,
                        state = %data["state"],
                        reason = %data["interrupt_reason"],
                        "download finished"
                    );
                }
                emit(topic, data);
            }
        }
    }
}
