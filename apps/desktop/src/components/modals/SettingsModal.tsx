// 设置弹窗: 分类 tab(通用/用户/安全/快捷键), 含头像压缩与快捷键捕获输入

import { useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { api } from "../../api";
import { getLocale, useI18n, type Lang } from "../../i18n";
import { AVATARS, avatarBlobUrl, type Settings } from "../../types";
import { Button, ModalShell, ToggleRow } from "./ModalShell";

/** 设置分类 tab(标签文案渲染时按语言取 t.settings.tabs) */
const TABS = ["general", "user", "security", "hotkey"] as const;
type TabKey = (typeof TABS)[number];

/** 界面语言选项 */
const LANGS: [Lang, string][] = [
  ["zh", "中文"],
  ["en", "English"],
];

/** 压缩头像: 居中裁方 → 128×128 → JPEG 字节(WebView 自带解码, 无需 Rust 图片库) */
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

/** 快捷键捕获输入: 聚焦后按组合键写入(必须含修饰键), Backspace 清除, Esc 取消 */
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
    <input
      readOnly
      value={recording ? t.settings.hotkeyRecording : (value ?? t.settings.hotkeyUnset)}
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
        // 主键限字母/数字/F1-F12, 且必须带修饰键(纯单键全局热键太易误触)
        const key = e.key.length === 1 ? e.key.toUpperCase() : e.key;
        if (mods.length === 0 || !/^([A-Z0-9]|F([1-9]|1[0-2]))$/.test(key)) return;
        onChange([...mods, key].join("+"));
        e.currentTarget.blur();
      }}
      className={`w-full cursor-pointer rounded-md border bg-abyss/60 px-3 py-1.5 font-gauge text-sm outline-none transition-colors ${
        recording ? "border-sonar/60 text-sonar" : "border-line-2 text-fog"
      }`}
    />
  );
}

/** 设置弹窗 */
export function SettingsModal({
  fingerprint,
  onSaved,
  onClose,
}: {
  fingerprint: string;
  onSaved: () => void;
  onClose: () => void;
}) {
  const { t, setLang } = useI18n();
  const [settings, setSettings] = useState<Settings | null>(null);
  const [tab, setTab] = useState<TabKey>("general");
  const [tip, setTip] = useState<string | null>(null);
  // 本机自定义头像预览(Blob URL)
  const [customPreview, setCustomPreview] = useState<string | null>(null);
  const fileRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    api.getSettings().then(setSettings).catch(console.error);
    // 已有自定义头像图片则加载预览
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

  /** 选图 → 压缩上传 → 选中自定义头像 */
  const onImagePicked = async (file: File | undefined) => {
    if (!file || !settings) return;
    try {
      const jpeg = await compressAvatar(file);
      await api.setAvatarImage(Array.from(jpeg));
      setCustomPreview(avatarBlobUrl(Array.from(jpeg)));
      setSettings({ ...settings, avatar: "custom" });
    } catch (e) {
      setTip(String(e));
    }
  };

  const save = async () => {
    if (!settings) return;
    try {
      await api.saveSettings({
        ...settings,
        // 空昵称视为跟随主机名
        displayName: settings.displayName?.trim() ? settings.displayName : null,
      });
      // 语言切换保存后即时生效(界面文案 + Rust 侧托盘/通知)
      if (settings.language === "zh" || settings.language === "en") {
        setLang(settings.language);
      }
      onSaved();
      onClose();
    } catch (e) {
      setTip(String(e));
    }
  };

  return (
    <ModalShell title={t.settings.title} onClose={onClose}>
      {/* 分类 tab 行 */}
      <div className="flex items-center border-b border-line px-3">
        {TABS.map((key) => (
          <button
            key={key}
            onClick={() => setTab(key)}
            className={`relative cursor-pointer px-3 py-2.5 text-xs font-medium tracking-[0.14em] transition-colors ${
              tab === key ? "text-fog" : "text-mist hover:text-fog/80"
            }`}
          >
            {t.settings.tabs[key]}
            {tab === key && (
              <span className="absolute inset-x-3 bottom-0 h-0.5 rounded-full bg-sonar" />
            )}
          </button>
        ))}
      </div>

      <div className="px-5 py-4">
        {settings ? (
          <>
            {tab === "general" && (
              <>
                <div className="gauge-label mb-1">{t.settings.downloadDir}</div>
                <div className="flex gap-2">
                  <input
                    readOnly
                    value={settings.downloadDir}
                    className="min-w-0 flex-1 rounded-md border border-line-2 bg-abyss/60 px-3 py-1.5 font-gauge text-xs text-fog/90 outline-none"
                  />
                  <Button onClick={pickDir}>{t.settings.choose}</Button>
                </div>

                <div className="gauge-label mt-4 mb-1">{t.settings.conflict}</div>
                <div className="flex gap-1.5">
                  {(
                    [
                      ["rename", t.settings.conflictRename],
                      ["overwrite", t.settings.conflictOverwrite],
                      ["ask", t.settings.conflictAsk],
                    ] as const
                  ).map(([value, label]) => (
                    <button
                      key={value}
                      onClick={() => setSettings({ ...settings, conflictPolicy: value })}
                      className={`cursor-pointer rounded-md border px-3 py-1.5 text-xs transition-colors ${
                        settings.conflictPolicy === value
                          ? "border-sonar/60 bg-sonar/15 text-sonar"
                          : "border-line-2 text-fog/70 hover:border-mist hover:text-fog"
                      }`}
                    >
                      {label}
                    </button>
                  ))}
                </div>

                <div className="gauge-label mt-4 mb-1">{t.settings.language}</div>
                <div className="flex gap-1.5">
                  {LANGS.map(([value, label]) => (
                    <button
                      key={value}
                      onClick={() => setSettings({ ...settings, language: value })}
                      className={`cursor-pointer rounded-md border px-3 py-1.5 text-xs transition-colors ${
                        settings.language === value
                          ? "border-sonar/60 bg-sonar/15 text-sonar"
                          : "border-line-2 text-fog/70 hover:border-mist hover:text-fog"
                      }`}
                    >
                      {label}
                    </button>
                  ))}
                </div>

                <div className="gauge-label mt-4 mb-1">{t.settings.port}</div>
                <input
                  type="number"
                  min={0}
                  max={65535}
                  value={settings.tcpPort}
                  onChange={(e) =>
                    setSettings({ ...settings, tcpPort: Number(e.target.value) || 0 })
                  }
                  className="w-32 rounded-md border border-line-2 bg-abyss/60 px-3 py-1.5 font-gauge text-sm text-fog outline-none focus:border-sonar/60"
                />

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
              </>
            )}

            {tab === "user" && (
              <>
                <div className="gauge-label mb-1">{t.settings.fingerprint}</div>
                <button
                  className="w-full cursor-pointer select-text truncate rounded-md border border-line/70 bg-abyss/50 px-3 py-1.5 text-left font-gauge text-[11px] text-mist transition-colors hover:text-fog"
                  title={t.settings.copyHint}
                  onClick={() => navigator.clipboard.writeText(fingerprint)}
                >
                  {fingerprint}
                </button>

                <div className="gauge-label mt-4 mb-1">{t.settings.nickname}</div>
                <input
                  value={settings.displayName ?? ""}
                  onChange={(e) => setSettings({ ...settings, displayName: e.target.value })}
                  placeholder={t.settings.nicknamePlaceholder}
                  className="w-full rounded-md border border-line-2 bg-abyss/60 px-3 py-1.5 text-sm text-fog outline-none focus:border-sonar/60"
                />

                <div className="gauge-label mt-4 mb-1">{t.settings.avatar}</div>
                <div className="flex flex-wrap gap-1.5">
                  {/* 首项 "Aa" 表示不用 emoji, 回退首字母样式 */}
                  {[null, ...AVATARS].map((a) => (
                    <button
                      key={a ?? "none"}
                      onClick={() => setSettings({ ...settings, avatar: a })}
                      title={a ? undefined : t.settings.initialStyle}
                      className={`flex size-9 cursor-pointer items-center justify-center rounded-md border text-base transition-colors ${
                        settings.avatar === a
                          ? "border-sonar/60 bg-sonar/15"
                          : "border-line-2 hover:border-mist"
                      }`}
                    >
                      {a ?? <span className="text-xs text-fog/70">Aa</span>}
                    </button>
                  ))}
                  {/* 自定义图片: 有图时点选启用, 无图或再次点击均打开选图 */}
                  <button
                    onClick={() => {
                      if (customPreview && settings.avatar !== "custom") {
                        setSettings({ ...settings, avatar: "custom" });
                      } else {
                        fileRef.current?.click();
                      }
                    }}
                    title={t.settings.uploadAvatar}
                    className={`flex size-9 cursor-pointer items-center justify-center overflow-hidden rounded-md border transition-colors ${
                      settings.avatar === "custom"
                        ? "border-sonar/60 bg-sonar/15"
                        : "border-line-2 hover:border-mist"
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
              </>
            )}

            {tab === "security" && (
              <>
                <div className="gauge-label mb-1">{t.settings.pin}</div>
                <input
                  value={settings.pin ?? ""}
                  onChange={(e) =>
                    setSettings({ ...settings, pin: e.target.value || null })
                  }
                  placeholder={t.settings.pinPlaceholder}
                  className="w-full rounded-md border border-line-2 bg-abyss/60 px-3 py-1.5 font-gauge text-sm text-fog outline-none focus:border-sonar/60"
                />

                {settings.trusted.length > 0 && (
                  <>
                    <div className="gauge-label mt-4 mb-1">{t.settings.trustedDevices}</div>
                    <div className="rounded-md border border-line/70 bg-abyss/50">
                      {settings.trusted.map((d) => (
                        <div
                          key={d.fingerprint}
                          className="flex items-center gap-3 border-b border-line/40 px-3 py-1.5 last:border-b-0"
                        >
                          <span className="min-w-0 flex-1 truncate text-xs text-fog/90">
                            {d.name}
                          </span>
                          <span className="font-gauge text-[10px] text-mist">
                            {d.fingerprint.slice(0, 8)}
                          </span>
                          <button
                            onClick={() =>
                              setSettings({
                                ...settings,
                                trusted: settings.trusted.filter(
                                  (x) => x.fingerprint !== d.fingerprint,
                                ),
                              })
                            }
                            className="cursor-pointer text-xs text-alert/80 transition-colors hover:text-alert"
                          >
                            {t.settings.remove}
                          </button>
                        </div>
                      ))}
                    </div>
                  </>
                )}

                <ToggleRow
                  label={t.settings.stealth}
                  checked={settings.passive}
                  onChange={(v) => setSettings({ ...settings, passive: v })}
                />
              </>
            )}

            {tab === "hotkey" && (
              <>
                <div className="gauge-label mb-1">{t.settings.hotkeyLabel}</div>
                <HotkeyInput
                  value={settings.sendClipboardHotkey}
                  onChange={(v) => setSettings({ ...settings, sendClipboardHotkey: v })}
                />
                <div className="mt-2 text-[11px] text-mist">{t.settings.hotkeyHint}</div>
              </>
            )}

            {/* 吸底操作栏: 各分类共享;
                毛玻璃模式下 bg-panel 是半透明变量, 必须叠 backdrop-blur
                把滚动上来的内容模糊掉, 否则文字会从按钮底下透出 */}
            <div className="sticky bottom-0 -mx-5 -mb-4 mt-5 flex items-center justify-end gap-3 border-t border-line bg-panel px-5 py-3 backdrop-blur-xl">
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
      </div>
    </ModalShell>
  );
}
