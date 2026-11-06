//! Operating-system notifications (Windows toasts) and "is our window in front?".
//!
//! Toasts use PowerShell's AppUserModelID, the documented way for an app that was not
//! installed with a Start-menu shortcut: no installer, no registry entries, no admin rights.
//! The toast then names "Windows PowerShell" as its sender. Toasts are informational only
//! (no buttons): questions say where to answer. Other platforms: not supported yet (the
//! channel reports an error, nothing else breaks).

use crate::model::{Level, Notification, truncate};

/// What a toast shows. Built separately from showing it, so it can be unit-tested.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OsToast {
    pub title: String,
    pub body: String,
    /// Play the default sound (warnings and errors only, and questions).
    pub sound: bool,
}

/// Toast text for a notification. Windows shows about 2 lines of title and 4 of text.
pub fn toast_for(n: &Notification) -> OsToast {
    let mut body = n.body.clone();
    if n.is_question() {
        let choices: Vec<&str> = n
            .actions
            .iter()
            .filter(|a| !a.remote_only)
            .map(|a| a.label.as_str())
            .collect();
        if !body.is_empty() {
            body.push('\n');
        }
        body.push_str(&format!("Answer in BrowserMCP: {}", choices.join(" / ")));
    }
    OsToast {
        title: truncate(&n.title, 120),
        body: truncate(&body, 300),
        sound: n.is_question() || matches!(n.level, Level::Warning | Level::Error),
    }
}

/// Show a toast. Blocking (COM calls): run it on a blocking thread.
#[cfg(windows)]
pub fn show(toast: &OsToast) -> Result<(), String> {
    use tauri_winrt_notification::{Sound, Toast};
    Toast::new(Toast::POWERSHELL_APP_ID)
        .title(&toast.title)
        .text1(&toast.body)
        .sound(if toast.sound {
            Some(Sound::Default)
        } else {
            None
        })
        .show()
        .map_err(|e| format!("windows toast: {e}"))
}

#[cfg(not(windows))]
pub fn show(_toast: &OsToast) -> Result<(), String> {
    Err("OS notifications are only implemented on Windows".into())
}

/// True when the foreground window belongs to this process (the browser is in front).
#[cfg(windows)]
pub fn app_is_foreground() -> bool {
    use windows_sys::Win32::System::Threading::GetCurrentProcessId;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindowThreadProcessId,
    };
    // SAFETY: plain Win32 queries without pointers we own (except `pid`, a local).
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_null() {
            return false;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut pid);
        pid == GetCurrentProcessId()
    }
}

/// Unknown elsewhere: assume "not in front" so `unfocused` rules still notify.
#[cfg(not(windows))]
pub fn app_is_foreground() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Action;

    #[test]
    fn toast_payload() {
        let n = Notification::new("Run r1 finished", "10/10 rows ok", Level::Success);
        assert_eq!(
            toast_for(&n),
            OsToast {
                title: "Run r1 finished".into(),
                body: "10/10 rows ok".into(),
                sound: false,
            }
        );

        let mut q = Notification::new("Run r1 needs a fix", "Row 3 failed", Level::Warning);
        q.actions = vec![
            Action::new("skip", "Skip row"),
            Action::new("stop", "Stop run"),
            Action::new("open", "Open in browser").remote_only(),
        ];
        let t = toast_for(&q);
        assert_eq!(
            t.body,
            "Row 3 failed\nAnswer in BrowserMCP: Skip row / Stop run"
        );
        assert!(t.sound);

        let long = Notification::new("x".repeat(500), "y".repeat(500), Level::Info);
        let t = toast_for(&long);
        assert_eq!(t.title.chars().count(), 120);
        assert_eq!(t.body.chars().count(), 300);
    }

    /// Shows one real toast. Manual: `cargo test -p bmcp-notify -- --ignored live_os_toast`.
    #[test]
    #[ignore = "shows a real Windows notification"]
    fn live_os_toast() {
        let n = Notification::new(
            "BrowserMCP test",
            "If you can read this, OS notifications work.",
            Level::Info,
        );
        show(&toast_for(&n)).unwrap();
    }
}
