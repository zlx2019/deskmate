//! Clipboard file-list reader: extracts absolute paths for files copied to the
//! system clipboard so hotkey sends can reuse the existing file-transfer flow.
//!
//! The Tauri clipboard plugin only supports text and images. File references
//! use platform-specific formats: macOS `public.file-url` and Windows
//! `CF_HDROP`. Linux is currently unsupported and returns an empty list.

/// Reads file paths copied to the clipboard.
///
/// Returns an empty list when no files are present or the platform is
/// unsupported. Paths that no longer exist are filtered out because the
/// clipboard may retain references to files deleted after they were copied.
pub fn read_file_paths() -> Vec<String> {
    platform::read_raw()
        .into_iter()
        .filter(|p| std::path::Path::new(p).exists())
        .collect()
}

/// macOS implementation: converts each NSPasteboard file URL to a filesystem path.
#[cfg(target_os = "macos")]
mod platform {
    use objc2_app_kit::{NSPasteboard, NSPasteboardTypeFileURL};
    use objc2_foundation::NSURL;

    pub fn read_raw() -> Vec<String> {
        // AppKit calls run on a blocking thread without an autorelease pool.
        objc2::rc::autoreleasepool(|_| {
            let pasteboard = NSPasteboard::generalPasteboard();
            let Some(items) = pasteboard.pasteboardItems() else {
                return Vec::new();
            };
            let mut out = Vec::new();
            for item in &items {
                let Some(url_str) = item.stringForType(unsafe { NSPasteboardTypeFileURL }) else {
                    continue;
                };
                // NSURL handles percent decoding for spaces and non-ASCII text.
                let Some(url) = NSURL::URLWithString(&url_str) else {
                    continue;
                };
                if let Some(path) = url.path() {
                    out.push(path.to_string());
                }
            }
            out
        })
    }
}

/// Windows implementation: expands paths from a CF_HDROP handle via DragQueryFileW.
#[cfg(target_os = "windows")]
mod platform {
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, GetClipboardData, OpenClipboard,
    };
    use windows_sys::Win32::UI::Shell::{DragQueryFileW, HDROP};

    /// CF_HDROP clipboard format ID for file lists, as defined by winuser.h.
    const CF_HDROP: u32 = 15;

    pub fn read_raw() -> Vec<String> {
        // The source application may briefly retain the clipboard after a copy.
        for attempt in 0..4 {
            if attempt > 0 {
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            if unsafe { OpenClipboard(std::ptr::null_mut()) } == 0 {
                continue;
            }
            // The HDROP handle is valid only while the clipboard remains open.
            let paths = read_hdrop();
            unsafe { CloseClipboard() };
            return paths;
        }
        Vec::new()
    }

    /// Expands every path in CF_HDROP. Requires an open clipboard.
    fn read_hdrop() -> Vec<String> {
        let handle = unsafe { GetClipboardData(CF_HDROP) };
        if handle.is_null() {
            return Vec::new();
        }
        let hdrop: HDROP = handle;
        // Passing u32::MAX as the file index queries the total file count.
        let count = unsafe { DragQueryFileW(hdrop, u32::MAX, std::ptr::null_mut(), 0) };
        let mut out = Vec::with_capacity(count as usize);
        for i in 0..count {
            // Query the path length first, excluding the terminator, then read it.
            let len = unsafe { DragQueryFileW(hdrop, i, std::ptr::null_mut(), 0) };
            if len == 0 {
                continue;
            }
            let mut buf = vec![0u16; len as usize + 1];
            let copied = unsafe { DragQueryFileW(hdrop, i, buf.as_mut_ptr(), buf.len() as u32) };
            if copied == 0 {
                continue;
            }
            out.push(String::from_utf16_lossy(&buf[..copied as usize]));
        }
        out
    }
}

/// Other platforms, including Linux, are currently unsupported.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod platform {
    pub fn read_raw() -> Vec<String> {
        Vec::new()
    }
}
