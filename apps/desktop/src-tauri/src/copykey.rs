//! System support for the copy-and-send hotkey. It simulates Cmd+C or Ctrl+C
//! and confirms the copy through the clipboard sequence number.
//!
//! Content comparison is avoided because large images are expensive to read,
//! and copying identical content is indistinguishable from no copy. The system
//! sequence number, NSPasteboard.changeCount on macOS and
//! GetClipboardSequenceNumber on Windows, increments after every write.
//!
//! An unchanged sequence means the foreground application did not copy,
//! usually because nothing was selected. In that case nothing is sent, which
//! prevents stale or sensitive clipboard content from being transmitted.
//!
//! Linux is currently unsupported: X11 injection needs another dependency and
//! Wayland has no portable solution.

/// Result of a simulated copy.
///
/// Each platform constructs only a subset of variants, while callers match all
/// variants and allow platform-specific unused-code warnings.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyOutcome {
    /// The clipboard changed and can be sent.
    Copied,
    /// The foreground application did not copy anything.
    NothingCopied,
    /// macOS accessibility permission is missing.
    PermissionNeeded,
    /// Simulated key presses are unsupported on this platform.
    Unsupported,
}

/// Simulates a system copy and waits for the clipboard sequence to change.
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub async fn copy_selection() -> CopyOutcome {
    use std::time::Duration;
    /// Clipboard sequence polling interval.
    const POLL_INTERVAL: Duration = Duration::from_millis(30);
    /// Total wait time for slower applications such as remote desktops or Office.
    const POLL_TIMEOUT: Duration = Duration::from_millis(600);

    if !platform::ensure_permission() {
        return CopyOutcome::PermissionNeeded;
    }
    let before = platform::clipboard_stamp();
    if !platform::send_copy_keystroke() {
        // Treat rare event-synthesis failures as no copied content.
        return CopyOutcome::NothingCopied;
    }
    let deadline = tokio::time::Instant::now() + POLL_TIMEOUT;
    while tokio::time::Instant::now() < deadline {
        tokio::time::sleep(POLL_INTERVAL).await;
        if platform::clipboard_stamp() != before {
            return CopyOutcome::Copied;
        }
    }
    CopyOutcome::NothingCopied
}

/// Reports unsupported on platforms without simulated copy support.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub async fn copy_selection() -> CopyOutcome {
    CopyOutcome::Unsupported
}

/// macOS implementation using CGEvent and NSPasteboard.changeCount.
#[cfg(target_os = "macos")]
mod platform {
    use objc2_core_foundation::{CFBoolean, CFDictionary, CFString, kCFBooleanTrue};
    use objc2_core_graphics::{CGEvent, CGEventFlags, CGEventTapLocation};

    /// Virtual key code for the physical C key, kVK_ANSI_C.
    const VK_C: u16 = 8;

    // objc2 has no binding for this ApplicationServices API, so declare it
    // using the same types as generated objc2 bindings.
    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        /// Option key that prompts for permission when set to true.
        static kAXTrustedCheckOptionPrompt: Option<&'static CFString>;
        /// Reports whether the process is trusted to synthesize keyboard events.
        fn AXIsProcessTrustedWithOptions(
            options: Option<&CFDictionary<CFString, CFBoolean>>,
        ) -> bool;
    }

    /// Checks accessibility permission and prompts once per process when missing.
    pub fn ensure_permission() -> bool {
        use std::sync::atomic::{AtomicBool, Ordering};
        static PROMPTED: AtomicBool = AtomicBool::new(false);
        // Use the no-prompt fast path for the common authorized case.
        if unsafe { AXIsProcessTrustedWithOptions(None) } {
            return true;
        }
        if !PROMPTED.swap(true, Ordering::Relaxed) {
            let (key, yes) = unsafe { (kAXTrustedCheckOptionPrompt, kCFBooleanTrue) };
            // Missing constants only prevent the guidance prompt.
            if let (Some(key), Some(yes)) = (key, yes) {
                let options = CFDictionary::from_slices(&[key], &[yes]);
                unsafe { AXIsProcessTrustedWithOptions(Some(&options)) };
            }
        }
        false
    }

    /// Current clipboard sequence number, incremented by every write.
    pub fn clipboard_stamp() -> i64 {
        // Tokio worker threads do not provide an AppKit autorelease pool.
        objc2::rc::autoreleasepool(|_| {
            objc2_app_kit::NSPasteboard::generalPasteboard().changeCount() as i64
        })
    }

    /// Synthesizes Cmd+C key-down and key-up events.
    ///
    /// Explicit Command-only flags prevent a physically held Shift key from
    /// altering the shortcut or recursively triggering this hotkey.
    pub fn send_copy_keystroke() -> bool {
        for key_down in [true, false] {
            let Some(event) = CGEvent::new_keyboard_event(None, VK_C, key_down) else {
                return false;
            };
            CGEvent::set_flags(Some(&event), CGEventFlags::MaskCommand);
            CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&event));
        }
        true
    }
}

/// Windows implementation using SendInput and GetClipboardSequenceNumber.
#[cfg(target_os = "windows")]
mod platform {
    use windows_sys::Win32::System::DataExchange::GetClipboardSequenceNumber;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput,
        VK_CONTROL, VK_LSHIFT, VK_RSHIFT,
    };

    /// Virtual key code for C, which has no named constant.
    const VK_C: u16 = 0x43;

    /// Windows input synthesis does not require permission.
    pub fn ensure_permission() -> bool {
        true
    }

    /// Current clipboard sequence number, incremented by every write.
    pub fn clipboard_stamp() -> i64 {
        i64::from(unsafe { GetClipboardSequenceNumber() })
    }

    /// Synthesizes Ctrl+C.
    ///
    /// SendInput merges injected and physical key states. Release held Shift
    /// keys first so the application receives Ctrl+C instead of Ctrl+Shift+C,
    /// then restore their current physical state. Release Ctrl only when it is
    /// no longer physically held.
    pub fn send_copy_keystroke() -> bool {
        /// Builds one keyboard input event.
        fn key(vk: u16, up: bool) -> INPUT {
            INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: vk,
                        wScan: 0,
                        dwFlags: if up { KEYEVENTF_KEYUP } else { 0 },
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            }
        }
        let held = |vk: u16| unsafe { GetAsyncKeyState(i32::from(vk)) } < 0;

        let (left_shift, right_shift) = (held(VK_LSHIFT), held(VK_RSHIFT));
        let mut seq: Vec<INPUT> = Vec::with_capacity(8);
        if left_shift {
            seq.push(key(VK_LSHIFT, true));
        }
        if right_shift {
            seq.push(key(VK_RSHIFT, true));
        }
        // Press Ctrl explicitly; a duplicate key-down is harmless.
        seq.push(key(VK_CONTROL, false));
        seq.push(key(VK_C, false));
        seq.push(key(VK_C, true));
        if !held(VK_CONTROL) {
            seq.push(key(VK_CONTROL, true));
        }
        // Restore Shift if it remains physically held.
        if left_shift && held(VK_LSHIFT) {
            seq.push(key(VK_LSHIFT, false));
        }
        if right_shift && held(VK_RSHIFT) {
            seq.push(key(VK_RSHIFT, false));
        }
        let sent = unsafe {
            SendInput(
                seq.len() as u32,
                seq.as_ptr(),
                std::mem::size_of::<INPUT>() as i32,
            )
        };
        sent == seq.len() as u32
    }
}
