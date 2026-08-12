// Settings dialog with categorized tabs, avatar compression, and hotkey capture.

import { Fragment, useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  Card as ACard,
  Input as AInput,
  Notification as ANotification,
  Radio as ARadio,
  Switch as ASwitch,
  Tabs as ATabs,
  Tag as ATag,
} from "animal-island-ui";
import { api } from "../../api";
import { formatError, getLocale, useI18n, type Lang } from "../../i18n";
import {
  PANEL_PRESETS,
  applyPanelColor,
  applyStyle,
  loadPanelColor,
  loadStyle,
  normalizeHex,
  savePanelColor,
  saveStyle,
  type StyleMode,
} from "../../theme";
import { AVATARS, avatarBlobUrl, avatarHashOf, type PeerDto, type Settings } from "../../types";
import { Avatar } from "../Radar";
import { Button, ModalShell, ToggleRow } from "./ModalShell";
import logoUrl from "../../assets/deskmate-logo.svg";

/** Settings categories whose labels resolve from t.settings.tabs at render time. */
const TABS = ["general", "user", "security", "hotkey", "ignore", "about"] as const;
type TabKey = (typeof TABS)[number];

/** Project repository URL used by the About page. */
const REPO_URL = "https://github.com/zlx2019/deskmate";
/** Component-library repository URL used by the About page. */
const UI_LIB_URL = "https://github.com/guokaigdg/animal-island-ui";

/** Interface language options. */
const LANGS: [Lang, string][] = [
  ["zh", "中文"],
  ["en", "English"],
];

/** Center-crops an avatar to 128 by 128 pixels and encodes JPEG in the WebView. */
async function compressAvatar(file: File): Promise<Uint8Array> {
  const bitmap = await createImageBitmap(file);
  const side = Math.min(bitmap.width, bitmap.height);
  const canvas = document.createElement("canvas");
  canvas.width = 128;
  canvas.height = 128;
  const ctx = canvas.getContext("2d");
  if (!ctx) throw new Error(getLocale().settings.canvasError);
  ctx.drawImage(
    bitmap,
    (bitmap.width - side) / 2,
    (bitmap.height - side) / 2,
    side,
    side,
    0,
    0,
    128,
    128,
  );
  bitmap.close();
  const blob = await new Promise<Blob | null>((r) => canvas.toBlob(r, "image/jpeg", 0.85));
  if (!blob) throw new Error(getLocale().settings.encodeError);
  return new Uint8Array(await blob.arrayBuffer());
}

/** Displays macOS modifier symbols while preserving cross-platform CmdOrCtrl input. */
const IS_MAC = navigator.userAgent.includes("Mac");

/** One rendered keycap label. */
function keyCapText(part: string): string {
  if (part === "CmdOrCtrl") return IS_MAC ? "⌘" : "Ctrl";
  if (part === "Shift") return IS_MAC ? "⇧" : "Shift";
  if (part === "Alt") return IS_MAC ? "⌥" : "Alt";
  return part;
}

/** Hotkey capture input requiring a modifier. Backspace clears and Escape cancels;
 * inactive mode renders the combination as individual keycaps. */
function HotkeyInput({
  value,
  onChange,
}: {
  value: string | null;
  onChange: (v: string | null) => void;
}) {
  const { t } = useI18n();
  const [recording, setRecording] = useState(false);
  return (
    <button
      type="button"
      onFocus={() => setRecording(true)}
      onBlur={() => setRecording(false)}
      onKeyDown={(e) => {
        e.preventDefault();
        if (e.key === "Escape") {
          e.currentTarget.blur();
          return;
        }
        if (e.key === "Backspace" || e.key === "Delete") {
          onChange(null);
          e.currentTarget.blur();
          return;
        }
        const mods: string[] = [];
        if (e.metaKey || e.ctrlKey) mods.push("CmdOrCtrl");
        if (e.altKey) mods.push("Alt");
        if (e.shiftKey) mods.push("Shift");
        // Restrict the main key to letters, digits, or F1-F12 and require a modifier.
        const key = e.key.length === 1 ? e.key.toUpperCase() : e.key;
        if (mods.length === 0 || !/^([A-Z0-9]|F([1-9]|1[0-2]))$/.test(key)) return;
        onChange([...mods, key].join("+"));
        e.currentTarget.blur();
      }}
      className={`flex min-h-8 min-w-40 shrink-0 cursor-pointer items-center justify-center gap-1 rounded-xl border-2 bg-panel-2 px-2.5 py-0.5 outline-none transition-colors ${
        recording ? "border-sonar" : "border-line hover:border-line-2"
      }`}
    >
      {recording ? (
        <span className="text-sm text-sonar">{t.settings.hotkeyRecording}</span>
      ) : value ? (
        value.split("+").map((part, i) => (
          <Fragment key={`${part}-${i}`}>
            {i > 0 && <span className="text-xs text-faint">+</span>}
            <kbd className="rounded-lg border-2 border-line bg-panel px-1.5 py-0.5 font-gauge text-xs font-bold text-fog shadow-[0_2px_0_var(--color-line-2)]">
              {keyCapText(part)}
            </kbd>
          </Fragment>
        ))
      ) : (
        <span className="text-sm text-faint">{t.settings.hotkeyUnset}</span>
      )}
    </button>
  );
}

/** Panel background selector with presets and custom hex input. It applies and
 * persists immediately in localStorage outside the settings save flow. */
function PanelColorPicker() {
  const [color, setColor] = useState(loadPanelColor);
  // Allow intermediate hex input and apply it as soon as it becomes valid.
  const [draft, setDraft] = useState(color);
  const pick = (hex: string) => {
    setColor(hex);
    setDraft(hex);
    applyPanelColor(hex);
    savePanelColor(hex);
  };
  const onDraftChange = (value: string) => {
    setDraft(value);
    const hex = normalizeHex(value);
    if (hex) {
      setColor(hex);
      applyPanelColor(hex);
      savePanelColor(hex);
    }
  };
  return (
    <div className="w-fit rounded-2xl border-2 border-line bg-panel-2/40 p-3">
      <div className="flex gap-2">
        {PANEL_PRESETS.map((c) => (
          <button
            key={c}
            onClick={() => pick(c)}
            style={{ background: c }}
            aria-label={c}
            title={c}
            className={`size-8 cursor-pointer rounded-full border-2 transition-transform hover:scale-110 ${
              color === c
                ? "border-sonar shadow-[0_0_0_2px_rgba(25,200,185,0.3)]"
                : "border-black/10 shadow-[inset_0_-2px_0_rgba(41,71,51,0.08)]"
            }`}
          />
        ))}
      </div>
      {/* Revert invalid custom-color drafts to the active value on blur. */}
      <input
        value={draft}
        onChange={(e) => onDraftChange(e.target.value)}
        onBlur={() => setDraft(normalizeHex(draft) ?? color)}
        spellCheck={false}
        className="mt-2.5 w-full rounded-xl border-2 border-line bg-panel px-3 py-1 text-center font-gauge text-xs text-fog outline-none transition-colors focus:border-sonar"
      />
    </div>
  );
}

/** Settings dialog. */
export function SettingsModal({
  fingerprint,
  peers,
  avatarSrcs,
  onSaved,
  onClose,
}: {
  fingerprint: string;
  /** Online peers used to enrich trusted-device cards with live avatars and state. */
  peers: PeerDto[];
  /** Image-avatar Blob URLs keyed by hash. */
  avatarSrcs: Record<string, string>;
  onSaved: () => void;
  onClose: () => void;
}) {
  const { t, setLang } = useI18n();
  const [settings, setSettings] = useState<Settings | null>(null);
  const [tab, setTab] = useState<TabKey>("general");
  const [tip, setTip] = useState<string | null>(null);
  // Interface style applies immediately and is stored in localStorage.
  const [styleMode, setStyleMode] = useState<StyleMode>(loadStyle);
  // Local custom-avatar preview Blob URL.
  const [customPreview, setCustomPreview] = useState<string | null>(null);
  const fileRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    api.getSettings().then(setSettings).catch(console.error);
    // Load a preview when a custom-avatar image already exists.
    api
      .getAvatarImage()
      .then((bytes) => bytes && bytes.length > 0 && setCustomPreview(avatarBlobUrl(bytes)))
      .catch(() => {});
  }, []);

  const pickDir = async () => {
    const dir = await open({ directory: true, title: t.settings.pickDirTitle });
    if (typeof dir === "string" && settings) {
      setSettings({ ...settings, downloadDir: dir });
    }
  };

  /** Selects, compresses, uploads, and activates a custom avatar. */
  const onImagePicked = async (file: File | undefined) => {
    if (!file || !settings) return;
    try {
      const jpeg = await compressAvatar(file);
      await api.setAvatarImage(Array.from(jpeg));
      setCustomPreview(avatarBlobUrl(Array.from(jpeg)));
      setSettings({ ...settings, avatar: "custom" });
    } catch (e) {
      setTip(formatError(e));
    }
  };

  const save = async () => {
    if (!settings) return;
    try {
      await api.saveSettings({
        ...settings,
        // Treat an empty display name as following the hostname.
        displayName: settings.displayName?.trim() ? settings.displayName : null,
      });
      // Saved language changes apply immediately to the UI, tray, and notifications.
      if (settings.language === "zh" || settings.language === "en") {
        setLang(settings.language);
      }
      // Resolve the success notice after switching so it uses the new locale.
      ANotification.success({ message: getLocale().settings.saved, duration: 2.5 });
      onSaved();
      onClose();
    } catch (e) {
      setTip(formatError(e));
    }
  };

  return (
    <ModalShell title={t.settings.title} onClose={onClose}>
      {settings ? (
        <>
          {/* Library Tabs mounts only the selected section; the save bar is shared below. */}
          <ATabs
            className="dm-tabs settings-tabs"
            activeKey={tab}
            onChange={(k) => setTab(k as TabKey)}
            aria-label={t.settings.title}
            items={[
              {
                key: "general",
                label: t.settings.tabs.general,
                children: (
                  <div className="px-5 py-4">
                <div className="gauge-label mb-1">{t.settings.downloadDir}</div>
                <div className="flex items-center gap-2">
                  <div className="min-w-0 flex-1">
                    <AInput readOnly size="small" value={settings.downloadDir} />
                  </div>
                  <Button onClick={pickDir}>{t.settings.choose}</Button>
                </div>

                <div className="gauge-label mt-4 mb-1">{t.settings.conflict}</div>
                <ARadio
                  size="small"
                  value={settings.conflictPolicy}
                  onChange={(v) =>
                    setSettings({
                      ...settings,
                      conflictPolicy: v as Settings["conflictPolicy"],
                    })
                  }
                  options={[
                    { label: t.settings.conflictRename, value: "rename" },
                    { label: t.settings.conflictOverwrite, value: "overwrite" },
                    { label: t.settings.conflictAsk, value: "ask" },
                  ]}
                />

                <div className="gauge-label mt-4 mb-1">{t.settings.language}</div>
                <ARadio
                  size="small"
                  value={settings.language ?? "zh"}
                  onChange={(v) => setSettings({ ...settings, language: v as Lang })}
                  options={LANGS.map(([value, label]) => ({ label, value }))}
                />

                <div className="gauge-label mt-4 mb-1">{t.settings.style}</div>
                {/* Day/night switch: on = day (light style), off = night (dark style). */}
                <div className="flex items-center gap-2.5">
                  <ASwitch
                    checked={styleMode === "light"}
                    onChange={(v) => {
                      const mode: StyleMode = v ? "light" : "dark";
                      setStyleMode(mode);
                      applyStyle(mode);
                      saveStyle(mode);
                    }}
                    checkedChildren="☀️"
                    unCheckedChildren="🌙"
                    aria-label={t.settings.style}
                  />
                  <span className="text-sm text-fog">
                    {styleMode === "dark" ? t.settings.styleDark : t.settings.styleLight}
                  </span>
                </div>

                {styleMode === "light" && (
                  <>
                    <div className="gauge-label mt-4 mb-1">{t.settings.panelColor}</div>
                    <PanelColorPicker />
                  </>
                )}

                <div className="gauge-label mt-4 mb-1">{t.settings.port}</div>
                <div className="w-32">
                  <AInput
                    type="number"
                    size="small"
                    min={0}
                    max={65535}
                    value={settings.tcpPort}
                    onChange={(e) =>
                      setSettings({ ...settings, tcpPort: Number(e.target.value) || 0 })
                    }
                  />
                </div>

                {/* Compact inline toggles that wrap when needed. */}
                <div className="flex flex-wrap items-center gap-x-8">
                  <ToggleRow
                    label={t.settings.autoCopy}
                    checked={settings.autoCopyText}
                    onChange={(v) => setSettings({ ...settings, autoCopyText: v })}
                  />
                  <ToggleRow
                    label={t.settings.autostart}
                    checked={settings.autostart}
                    onChange={(v) => setSettings({ ...settings, autostart: v })}
                  />
                  <ToggleRow
                    label={t.settings.stealth}
                    checked={settings.passive}
                    onChange={(v) => setSettings({ ...settings, passive: v })}
                  />
                </div>
                  </div>
                ),
              },
              {
                key: "user",
                label: t.settings.tabs.user,
                children: (
                  <div className="px-5 py-4">
                <div className="gauge-label mb-1">{t.settings.fingerprint}</div>
                <button
                  className="w-full cursor-pointer select-text truncate rounded-xl border-2 border-line bg-panel-2 px-3 py-1.5 text-left font-gauge text-[11px] text-mist transition-colors hover:text-fog"
                  title={t.settings.copyHint}
                  onClick={() => navigator.clipboard.writeText(fingerprint)}
                >
                  {fingerprint}
                </button>

                {/* PIN and nickname share one row; full width reads oversized. */}
                <div className="mt-4 grid grid-cols-2 gap-3">
                  <div>
                    <div className="gauge-label mb-1">{t.settings.pin}</div>
                    <AInput
                      size="small"
                      value={settings.pin ?? ""}
                      onChange={(e) =>
                        setSettings({ ...settings, pin: e.target.value || null })
                      }
                      placeholder={t.settings.pinPlaceholder}
                    />
                  </div>
                  <div>
                    <div className="gauge-label mb-1">{t.settings.nickname}</div>
                    <AInput
                      size="small"
                      value={settings.displayName ?? ""}
                      onChange={(e) =>
                        setSettings({ ...settings, displayName: e.target.value })
                      }
                      placeholder={t.settings.nicknamePlaceholder}
                    />
                  </div>
                </div>

                <div className="gauge-label mt-4 mb-1">{t.settings.avatar}</div>
                <div className="flex flex-wrap gap-1.5">
                  {/* "Aa" disables emoji and uses the initial style. */}
                  {[null, ...AVATARS].map((a) => (
                    <button
                      key={a ?? "none"}
                      onClick={() => setSettings({ ...settings, avatar: a })}
                      title={a ? undefined : t.settings.initialStyle}
                      className={`flex size-9 cursor-pointer items-center justify-center rounded-xl border-2 text-base transition-colors ${
                        settings.avatar === a
                          ? "border-sonar bg-sonar/12"
                          : "border-line bg-panel-2 hover:border-line-2"
                      }`}
                    >
                      {a ?? <span className="text-xs text-fog/70">Aa</span>}
                    </button>
                  ))}
                  {/* Custom image activates when available; otherwise clicking opens the picker. */}
                  <button
                    onClick={() => {
                      if (customPreview && settings.avatar !== "custom") {
                        setSettings({ ...settings, avatar: "custom" });
                      } else {
                        fileRef.current?.click();
                      }
                    }}
                    title={t.settings.uploadAvatar}
                    className={`flex size-9 cursor-pointer items-center justify-center overflow-hidden rounded-xl border-2 transition-colors ${
                      settings.avatar === "custom"
                        ? "border-sonar bg-sonar/12"
                        : "border-line bg-panel-2 hover:border-line-2"
                    }`}
                  >
                    {customPreview ? (
                      <img src={customPreview} alt="" className="size-full object-cover" />
                    ) : (
                      <span className="text-xs text-fog/70">📷</span>
                    )}
                  </button>
                  <input
                    ref={fileRef}
                    type="file"
                    accept="image/*"
                    className="hidden"
                    onChange={(e) => {
                      onImagePicked(e.target.files?.[0]);
                      e.target.value = "";
                    }}
                  />
                </div>
                  </div>
                ),
              },
              {
                key: "security",
                label: t.settings.tabs.security,
                children: (
                  <div className="px-5 py-4">
                {settings.trusted.length > 0 ? (
                  <>
                    <div className="gauge-label mb-2">{t.settings.trustedDevices}</div>
                    {settings.trusted.map((d) => {
                      // Use live avatar and online state when available; otherwise show initials.
                      const online = peers.find((p) => p.fingerprint === d.fingerprint);
                      const hash = avatarHashOf(online?.avatar);
                      return (
                        <ACard
                          key={d.fingerprint}
                          pattern="app-green"
                          className="transfer-card mt-2 first:mt-0"
                        >
                          <div className="flex items-center gap-3">
                            <div className="relative shrink-0">
                              <Avatar
                                name={d.name}
                                fingerprint={d.fingerprint}
                                size={36}
                                avatar={online?.avatar}
                                src={hash ? avatarSrcs[hash] : undefined}
                              />
                              {online && (
                                <span className="absolute -right-0.5 -bottom-0.5 size-2.5 rounded-full border-2 border-panel bg-live" />
                              )}
                            </div>
                            <div className="min-w-0 flex-1">
                              <div className="truncate text-sm font-bold text-fog">{d.name}</div>
                              <div className="truncate font-gauge text-[10px] text-mist">
                                {d.fingerprint.slice(0, 16)}
                              </div>
                            </div>
                            <Button
                              variant="danger"
                              onClick={() =>
                                setSettings({
                                  ...settings,
                                  trusted: settings.trusted.filter(
                                    (x) => x.fingerprint !== d.fingerprint,
                                  ),
                                })
                              }
                            >
                              {t.settings.remove}
                            </Button>
                          </div>
                        </ACard>
                      );
                    })}
                  </>
                ) : (
                  <div className="py-8 text-center text-xs text-mist/70">
                    {t.settings.trustedEmpty}
                  </div>
                )}
                  </div>
                ),
              },
              {
                key: "hotkey",
                label: t.settings.tabs.hotkey,
                children: (
                  <div className="px-5 py-4">
                {/* Row list with labels and content-sized keycap inputs. Add future
                    hotkeys by appending another item here. */}
                {(
                  [
                    { field: "sendClipboardHotkey", label: t.settings.hotkeyLabel },
                    { field: "copySendHotkey", label: t.settings.copyHotkeyLabel },
                  ] as const
                ).map(({ field, label }) => (
                  <div key={field} className="mt-2 flex items-center gap-3 first:mt-0">
                    <div className="min-w-0 flex-1 text-sm text-fog">{label}</div>
                    <HotkeyInput
                      value={settings[field]}
                      onChange={(v) => setSettings({ ...settings, [field]: v })}
                    />
                  </div>
                ))}
                <div className="mt-4 text-[11px] text-mist">{t.settings.hotkeyHint}</div>
                  </div>
                ),
              },
              {
                key: "ignore",
                label: t.settings.tabs.ignore,
                children: (
                  <div className="px-5 py-4">
                <div className="gauge-label mb-1">{t.settings.ignoreRules}</div>
                <textarea
                  value={settings.ignoreRules}
                  onChange={(e) => setSettings({ ...settings, ignoreRules: e.target.value })}
                  placeholder={t.settings.ignoreRulesPlaceholder}
                  rows={10}
                  spellCheck={false}
                  className="w-full resize-y rounded-xl border-2 border-line bg-panel-2 px-3 py-2 font-gauge text-sm leading-relaxed text-fog outline-none transition-colors placeholder:text-faint focus:border-sonar"
                />
                <div className="mt-2 text-[11px] text-mist">{t.settings.ignoreRulesHint}</div>
                  </div>
                ),
              },
              {
                key: "about",
                label: t.settings.tabs.about,
                children: (
                  <div className="flex h-full flex-col items-center justify-center px-5 py-4 text-center">
                    <img src={logoUrl} alt="Deskmate" className="size-16" />
                    <div className="mt-3 flex items-center gap-2">
                      <span className="text-lg font-bold text-fog">Deskmate</span>
                      <ATag size="small" variant="soft" color="app-teal">
                        v{__APP_VERSION__}
                      </ATag>
                    </div>
                    <p className="mt-2 text-xs leading-relaxed text-mist">
                      {t.settings.aboutSlogan}
                    </p>
                    <div className="mt-5">
                      <Button onClick={() => openUrl(REPO_URL).catch(console.error)}>
                        GitHub
                      </Button>
                    </div>
                    <div className="mt-5 text-[11px] text-faint">
                      {t.settings.aboutCredit}{" "}
                      <button
                        className="cursor-pointer underline decoration-dotted underline-offset-2 transition-colors hover:text-mist"
                        onClick={() => openUrl(UI_LIB_URL).catch(console.error)}
                      >
                        animal-island-ui
                      </button>
                    </div>
                  </div>
                ),
              },
            ]}
          />

          {/* Shared sticky action bar with an opaque panel background. */}
          <div className="sticky bottom-0 flex items-center justify-end gap-3 border-t border-line bg-panel px-5 py-3">
            {tip && <span className="max-w-52 truncate text-xs text-alert">{tip}</span>}
            <Button onClick={onClose}>{t.settings.cancel}</Button>
            <Button variant="primary" onClick={save}>
              {t.settings.save}
            </Button>
          </div>
        </>
      ) : (
        <div className="py-6 text-center text-xs text-mist">{t.settings.loading}</div>
      )}
    </ModalShell>
  );
}
