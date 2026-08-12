//! Application settings persisted to settings.json in the data directory.
//!
//! Only the listening port requires a restart because the socket is fixed at
//! startup. All other settings take effect immediately.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Settings file name.
const SETTINGS_FILE: &str = "settings.json";
/// Default ignore rules written on first launch: IDE/AI tool directories,
/// dependency and build outputs, secret files, and OS metadata junk. Users
/// may edit or clear them; a cleared value persists as an empty string that
/// deserialization keeps as-is, so removals are never refilled.
///
/// The .vscode exceptions need `**/` prefixes: slash-containing patterns
/// anchor to the manifest root (whose first segment is the selected
/// directory name), and pruning `.vscode/` outright would void the inner
/// negations, matching git semantics.
pub const DEFAULT_IGNORE_RULES: &str = "\
.idea/
**/.vscode/*
!**/.vscode/settings.json
!**/.vscode/tasks.json
!**/.vscode/launch.json
!**/.vscode/extensions.json

.claude/
.codex/
.cursor/

node_modules/
target/
dist/
build/
.venv/
vendor/
__pycache__/
.cache/
.pytest_cache/

.env
.env.local
.env.*.local
*.pem
*.key

.DS_Store
Thumbs.db";
/// Local custom-avatar JPEG file name in the data directory.
pub const AVATAR_FILE: &str = "avatar.jpg";
/// Special settings.avatar value selecting a custom image instead of an emoji.
pub const AVATAR_CUSTOM: &str = "custom";

/// Application-level file-name conflict policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ConflictPolicySetting {
    /// Renames automatically, for example `file.txt` to `file (1).txt`.
    #[default]
    Rename,
    /// Overwrites the existing file.
    Overwrite,
    /// Asks in each incoming-offer dialog.
    Ask,
}

/// Trusted device whose transfers are accepted without confirmation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrustedDevice {
    /// Certificate fingerprint used as the device identity.
    pub fingerprint: String,
    /// Display name captured when trusted; later renames do not affect trust.
    pub name: String,
}

/// User settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// Custom display name; None follows the hostname.
    pub display_name: Option<String>,
    /// Default download directory.
    pub download_dir: PathBuf,
    /// TCP listening port; zero selects a random port.
    pub tcp_port: u16,
    /// File-name conflict policy.
    pub conflict_policy: ConflictPolicySetting,
    /// Avatar: emoji, [`AVATAR_CUSTOM`], or None for the initial style.
    pub avatar: Option<String>,
    /// Passive mode discovers peers without advertising this device.
    pub passive: bool,
    /// Launch at system startup.
    pub autostart: bool,
    /// Trusted-device allowlist for automatic acceptance.
    pub trusted: Vec<TrustedDevice>,
    /// Optional pairing PIN required for incoming files and text.
    pub pin: Option<String>,
    /// Automatically copies received text to the system clipboard.
    pub auto_copy_text: bool,
    /// Global send-clipboard hotkey in Tauri syntax; None disables it.
    pub send_clipboard_hotkey: Option<String>,
    /// Global copy-and-send hotkey. It simulates copying the selection, waits
    /// for clipboard confirmation, then uses the send-clipboard flow.
    pub copy_send_hotkey: Option<String>,
    /// Gitignore-style transfer rules, one per line. Matching files are omitted
    /// from send manifests, and syntax is validated when settings are saved.
    /// Seeded with [`DEFAULT_IGNORE_RULES`] on first launch; users may edit or clear.
    pub ignore_rules: String,
    /// Interface language: "zh" or "en"; empty until first-run system detection.
    pub language: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            display_name: None,
            download_dir: default_download_dir(),
            tcp_port: deskmate_core::DEFAULT_TCP_PORT,
            conflict_policy: ConflictPolicySetting::default(),
            avatar: None,
            passive: false,
            autostart: false,
            trusted: Vec::new(),
            pin: None,
            auto_copy_text: false,
            send_clipboard_hotkey: Some("CmdOrCtrl+Shift+D".to_string()),
            copy_send_hotkey: Some("CmdOrCtrl+Shift+X".to_string()),
            ignore_rules: DEFAULT_IGNORE_RULES.to_string(),
            language: String::new(),
        }
    }
}

impl Settings {
    /// Loads settings from the data directory, falling back on missing or invalid data.
    pub fn load(data_dir: &Path) -> Self {
        std::fs::read(data_dir.join(SETTINGS_FILE))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    /// Persists settings to the data directory.
    pub fn save(&self, data_dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(data_dir)?;
        let json = serde_json::to_vec_pretty(self).unwrap_or_default();
        std::fs::write(data_dir.join(SETTINGS_FILE), json)
    }
}

/// Default download directory: ~/Downloads/Deskmate.
fn default_download_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Downloads")
        .join("Deskmate")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ensures newly added fields default cleanly when loading older settings.
    #[test]
    fn missing_fields_fall_back_to_defaults() {
        let s: Settings = serde_json::from_str("{}").expect("an empty object should parse");
        assert_eq!(
            s.send_clipboard_hotkey.as_deref(),
            Some("CmdOrCtrl+Shift+D")
        );
        assert_eq!(s.copy_send_hotkey.as_deref(), Some("CmdOrCtrl+Shift+X"));
        assert_eq!(s.ignore_rules, DEFAULT_IGNORE_RULES);
    }

    /// Clearing the rules persists an empty string; present fields must never be refilled.
    #[test]
    fn cleared_ignore_rules_stay_empty() {
        let s: Settings =
            serde_json::from_str(r#"{"ignoreRules": ""}"#).expect("empty rules should parse");
        assert!(s.ignore_rules.is_empty());
    }

    /// Default rules must be valid gitignore syntax, or the first-run settings
    /// save would be rejected by validation.
    #[test]
    fn default_ignore_rules_are_valid() {
        deskmate_core::transfer::IgnoreRules::parse(DEFAULT_IGNORE_RULES)
            .expect("default rules should pass syntax validation");
    }

    /// Behavioral intent of the defaults: dependencies, builds, and secrets are
    /// filtered while .vscode shared configs survive. Guards the `**/` prefix
    /// form — pruning `.vscode/` outright would void the negations.
    #[test]
    fn default_ignore_rules_behavior() {
        let rules = deskmate_core::transfer::IgnoreRules::parse(DEFAULT_IGNORE_RULES)
            .expect("default rules should pass syntax validation");
        let root = std::env::temp_dir().join(format!("dm-default-rules-{}", std::process::id()));
        let proj = root.join("proj");
        for dir in ["src", ".vscode", "node_modules", "target"] {
            std::fs::create_dir_all(proj.join(dir)).expect("failed to create directory");
        }
        for file in [
            "src/main.rs",
            ".vscode/settings.json",
            ".vscode/other.json",
            "node_modules/x.js",
            "target/out.bin",
            ".env",
            "cert.pem",
        ] {
            std::fs::write(proj.join(file), b"x").expect("failed to write file");
        }

        let files = deskmate_core::transfer::collect_files(&[proj], Some(&rules))
            .expect("manifest should not be empty");
        let mut rels: Vec<String> = files.into_iter().map(|(_, rel, _)| rel).collect();
        rels.sort();
        std::fs::remove_dir_all(&root).ok();

        assert_eq!(
            rels,
            ["proj/.vscode/settings.json", "proj/src/main.rs"],
            "only sources and .vscode shared configs should remain"
        );
    }
}
