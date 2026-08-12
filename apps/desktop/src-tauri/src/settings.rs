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

/// Received-content kinds eligible for automatic clipboard copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AutoCopyKind {
    /// Text messages, written as plain text.
    Text,
    /// Inline clipboard screenshots, written as bitmap images.
    Image,
    /// Received files, written as pasteable file references.
    File,
    /// Received directories, written as pasteable file references.
    Dir,
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
    /// Legacy master switch kept only to migrate stored settings; the kind
    /// list below is the sole control now. [`Settings::load`] folds a stored
    /// `false` into an empty kind list and then pins this field to `true`.
    pub auto_copy_text: bool,
    /// Received-content kinds copied to the clipboard; empty disables auto
    /// copy. The default matches the pre-split behavior of the master switch.
    pub auto_copy_kinds: Vec<AutoCopyKind>,
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
            auto_copy_kinds: vec![AutoCopyKind::Text],
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
        let mut s: Self = std::fs::read(data_dir.join(SETTINGS_FILE))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        s.migrate_auto_copy();
        s
    }

    /// Folds the retired auto-copy master switch into the kind list.
    ///
    /// A stored `false` (also the fresh-install default) clears the kinds so
    /// upgrades never enable copying that was off; afterwards the flag stays
    /// pinned to `true` and the kind list alone controls the feature.
    fn migrate_auto_copy(&mut self) {
        if !self.auto_copy_text {
            self.auto_copy_kinds.clear();
            self.auto_copy_text = true;
        }
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
        // Pre-1.5 users with the switch on copied text; the default keeps that.
        assert_eq!(s.auto_copy_kinds, vec![AutoCopyKind::Text]);
    }

    /// The retired master switch folds into the kind list exactly once.
    #[test]
    fn auto_copy_migration_respects_old_switch() {
        // Switch off (or fresh install): kinds clear so nothing turns on.
        let mut off: Settings =
            serde_json::from_str(r#"{"autoCopyText":false}"#).expect("legacy off should parse");
        off.migrate_auto_copy();
        assert!(off.auto_copy_kinds.is_empty());
        assert!(off.auto_copy_text);

        // Switch on without a kind list: the text default carries over.
        let mut on: Settings =
            serde_json::from_str(r#"{"autoCopyText":true}"#).expect("legacy on should parse");
        on.migrate_auto_copy();
        assert_eq!(on.auto_copy_kinds, vec![AutoCopyKind::Text]);

        // Migrated settings with kinds explicitly cleared stay cleared.
        let mut cleared: Settings =
            serde_json::from_str(r#"{"autoCopyText":true,"autoCopyKinds":[]}"#)
                .expect("cleared kinds should parse");
        cleared.migrate_auto_copy();
        assert!(cleared.auto_copy_kinds.is_empty());
    }

    /// The kind list round-trips through lowercase JSON values.
    #[test]
    fn auto_copy_kinds_roundtrip() {
        let s: Settings = serde_json::from_str(r#"{"autoCopyKinds":["image","file","dir"]}"#)
            .expect("kind list should parse");
        assert_eq!(
            s.auto_copy_kinds,
            vec![AutoCopyKind::Image, AutoCopyKind::File, AutoCopyKind::Dir]
        );
        let json = serde_json::to_string(&s).expect("settings should serialize");
        assert!(json.contains(r#""autoCopyKinds":["image","file","dir"]"#));
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
