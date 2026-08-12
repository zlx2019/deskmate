//! Clipboard file-list reader/writer: file references copied to or from the
//! system clipboard, shared by hotkey sends and receive-side auto copy.
//!
//! The Tauri clipboard plugin only supports text and images. File references
//! use platform-specific formats: macOS `public.file-url` and Windows
//! `CF_HDROP`. Linux is currently unsupported: reads return an empty list and
//! writes report failure.

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

/// Writes pasteable file references to the clipboard, replacing its contents.
///
/// Pasting in a file manager copies the referenced files, so the originals in
/// the download directory remain. Returns false when nothing was written or
/// the platform is unsupported.
pub fn write_file_paths(paths: &[std::path::PathBuf]) -> bool {
    if paths.is_empty() {
        return false;
    }
    platform::write_raw(paths)
}

/// macOS implementation: converts each NSPasteboard file URL to a filesystem path.
#[cfg(target_os = "macos")]
mod platform {
    use objc2::runtime::ProtocolObject;
    use objc2_app_kit::{NSPasteboard, NSPasteboardTypeFileURL, NSPasteboardWriting};
    use objc2_foundation::{NSArray, NSString, NSURL};

    /// Replaces the clipboard with file URLs via writeObjects.
    pub fn write_raw(paths: &[std::path::PathBuf]) -> bool {
        objc2::rc::autoreleasepool(|_| {
            let urls: Vec<_> = paths
                .iter()
                .map(|p| {
                    let url = NSURL::fileURLWithPath(&NSString::from_str(&p.to_string_lossy()));
                    ProtocolObject::<dyn NSPasteboardWriting>::from_retained(url)
                })
                .collect();
            let pasteboard = NSPasteboard::generalPasteboard();
            pasteboard.clearContents();
            pasteboard.writeObjects(&NSArray::from_retained_slice(&urls))
        })
    }

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
        CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData,
    };
    use windows_sys::Win32::System::Memory::{
        GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock,
    };
    // GlobalFree lives in Foundation in windows-sys 0.61, not System::Memory.
    use windows_sys::Win32::Foundation::GlobalFree;
    use windows_sys::Win32::UI::Shell::{DROPFILES, DragQueryFileW, HDROP};

    /// CF_HDROP clipboard format ID for file lists, as defined by winuser.h.
    const CF_HDROP: u32 = 15;

    /// Replaces the clipboard with a CF_HDROP file list.
    pub fn write_raw(paths: &[std::path::PathBuf]) -> bool {
        use std::os::windows::ffi::OsStrExt;
        // Wide path list: each path null-terminated, list double-terminated.
        let mut wide: Vec<u16> = Vec::new();
        for p in paths {
            wide.extend(p.as_os_str().encode_wide());
            wide.push(0);
        }
        wide.push(0);

        let header = std::mem::size_of::<DROPFILES>();
        let total = header + wide.len() * 2;
        unsafe {
            let handle = GlobalAlloc(GMEM_MOVEABLE, total);
            if handle.is_null() {
                return false;
            }
            let base = GlobalLock(handle);
            if base.is_null() {
                GlobalFree(handle);
                return false;
            }
            let drop = base.cast::<DROPFILES>();
            (*drop).pFiles = header as u32;
            (*drop).pt.x = 0;
            (*drop).pt.y = 0;
            (*drop).fNC = 0;
            // fWide selects the UTF-16 list layout written below.
            (*drop).fWide = 1;
            std::ptr::copy_nonoverlapping(
                wide.as_ptr(),
                base.cast::<u8>().add(header).cast::<u16>(),
                wide.len(),
            );
            GlobalUnlock(handle);

            // The source application may briefly hold the clipboard open.
            for attempt in 0..4 {
                if attempt > 0 {
                    std::thread::sleep(std::time::Duration::from_millis(25));
                }
                if OpenClipboard(std::ptr::null_mut()) == 0 {
                    continue;
                }
                EmptyClipboard();
                // On success the system owns the memory; free it only on failure.
                let ok = !SetClipboardData(CF_HDROP, handle).is_null();
                CloseClipboard();
                if !ok {
                    GlobalFree(handle);
                }
                return ok;
            }
            GlobalFree(handle);
            false
        }
    }

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

    pub fn write_raw(_paths: &[std::path::PathBuf]) -> bool {
        false
    }
}
