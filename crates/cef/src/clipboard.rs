//! Plain text to the system clipboard ("Copy link address"). CEF has no clipboard API, and
//! a page script would need a user gesture, so this uses Win32 directly.

/// Put `text` on the clipboard. Returns false if that failed (or on other platforms).
#[cfg(windows)]
pub(crate) fn set_text(text: &str) -> bool {
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
    };
    use windows_sys::Win32::System::Memory::{
        GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock,
    };
    /// `CF_UNICODETEXT`: UTF-16, NUL-terminated.
    const CF_UNICODETEXT: u32 = 13;

    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes = wide.len() * std::mem::size_of::<u16>();
    // SAFETY: plain Win32 clipboard calls. The memory block is allocated, filled while
    // locked, and handed to the clipboard (which then owns it); on failure it is not used.
    unsafe {
        if OpenClipboard(std::ptr::null_mut()) == 0 {
            return false;
        }
        let mut ok = false;
        if EmptyClipboard() != 0 {
            let block = GlobalAlloc(GMEM_MOVEABLE, bytes);
            if !block.is_null() {
                let target = GlobalLock(block) as *mut u16;
                if !target.is_null() {
                    std::ptr::copy_nonoverlapping(wide.as_ptr(), target, wide.len());
                    GlobalUnlock(block);
                    ok = !SetClipboardData(CF_UNICODETEXT, block).is_null();
                }
            }
        }
        CloseClipboard();
        ok
    }
}

#[cfg(not(windows))]
pub(crate) fn set_text(_text: &str) -> bool {
    false
}
