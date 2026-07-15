// 设置弹窗: 昵称/头像/下载目录/冲突策略/端口/受信设备/配对 PIN/隐身/自启, 含头像压缩工具

import { useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { api } from "../../api";
import { AVATARS, avatarBlobUrl, type Settings } from "../../types";
import { Button, ModalShell, ToggleRow } from "./ModalShell";

/** 压缩头像: 居中裁方 → 128×128 → JPEG 字节(WebView 自带解码, 无需 Rust 图片库) */
async function compressAvatar(file: File): Promise<Uint8Array> {
  const bitmap = await createImageBitmap(file);
  const side = Math.min(bitmap.width, bitmap.height);
  const canvas = document.createElement("canvas");
  canvas.width = 128;
  canvas.height = 128;
  const ctx = canvas.getContext("2d");
  if (!ctx) throw new Error("Canvas 不可用");
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
  if (!blob) throw new Error("图片编码失败");
  return new Uint8Array(await blob.arrayBuffer());
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
  const [settings, setSettings] = useState<Settings | null>(null);
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
    const dir = await open({ directory: true, title: "选择默认下载目录" });
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
      onSaved();
      onClose();
    } catch (e) {
      setTip(String(e));
    }
  };

  return (
    <ModalShell title="settings" onClose={onClose}>
      <div className="px-5 py-4">
        <div className="gauge-label mb-1">device fingerprint</div>
        <button
          className="w-full cursor-pointer select-text truncate rounded-md border border-line/70 bg-abyss/50 px-3 py-1.5 text-left font-gauge text-[11px] text-mist transition-colors hover:text-fog"
          title="点击复制"
          onClick={() => navigator.clipboard.writeText(fingerprint)}
        >
          {fingerprint}
        </button>

        {settings ? (
          <>
            <div className="gauge-label mt-4 mb-1">昵称</div>
            <input
              value={settings.displayName ?? ""}
              onChange={(e) => setSettings({ ...settings, displayName: e.target.value })}
              placeholder="留空则使用主机名"
              className="w-full rounded-md border border-line-2 bg-abyss/60 px-3 py-1.5 text-sm text-fog outline-none focus:border-sonar/60"
            />

            <div className="gauge-label mt-4 mb-1">头像</div>
            <div className="flex flex-wrap gap-1.5">
              {/* 首项 "Aa" 表示不用 emoji, 回退首字母样式 */}
              {[null, ...AVATARS].map((a) => (
                <button
                  key={a ?? "none"}
                  onClick={() => setSettings({ ...settings, avatar: a })}
                  title={a ? undefined : "首字母样式"}
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
                title="上传图片头像"
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

            <div className="gauge-label mt-4 mb-1">下载目录</div>
            <div className="flex gap-2">
              <input
                readOnly
                value={settings.downloadDir}
                className="min-w-0 flex-1 rounded-md border border-line-2 bg-abyss/60 px-3 py-1.5 font-gauge text-xs text-fog/90 outline-none"
              />
              <Button onClick={pickDir}>选择…</Button>
            </div>

            <div className="gauge-label mt-4 mb-1">同名文件处理</div>
            <div className="flex gap-1.5">
              {(
                [
                  ["rename", "自动重命名"],
                  ["overwrite", "覆盖"],
                  ["ask", "每次询问"],
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

            <div className="gauge-label mt-4 mb-1">监听端口</div>
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

            {settings.trusted.length > 0 && (
              <>
                <div className="gauge-label mt-4 mb-1">受信设备(免确认自动接收)</div>
                <div className="rounded-md border border-line/70 bg-abyss/50">
                  {settings.trusted.map((t) => (
                    <div
                      key={t.fingerprint}
                      className="flex items-center gap-3 border-b border-line/40 px-3 py-1.5 last:border-b-0"
                    >
                      <span className="min-w-0 flex-1 truncate text-xs text-fog/90">
                        {t.name}
                      </span>
                      <span className="font-gauge text-[10px] text-mist">
                        {t.fingerprint.slice(0, 8)}
                      </span>
                      <button
                        onClick={() =>
                          setSettings({
                            ...settings,
                            trusted: settings.trusted.filter(
                              (x) => x.fingerprint !== t.fingerprint,
                            ),
                          })
                        }
                        className="cursor-pointer text-xs text-alert/80 transition-colors hover:text-alert"
                      >
                        移除
                      </button>
                    </div>
                  ))}
                </div>
              </>
            )}

            <div className="gauge-label mt-4 mb-1">配对 PIN</div>
            <input
              value={settings.pin ?? ""}
              onChange={(e) =>
                setSettings({ ...settings, pin: e.target.value || null })
              }
              placeholder="留空关闭; 启用后对方发送必须输入此 PIN"
              className="w-full rounded-md border border-line-2 bg-abyss/60 px-3 py-1.5 font-gauge text-sm text-fog outline-none focus:border-sonar/60"
            />

            <ToggleRow
              label="自动复制"
              checked={settings.autoCopyText}
              onChange={(v) => setSettings({ ...settings, autoCopyText: v })}
            />
            <ToggleRow
              label="隐身模式"
              hint="别人的雷达看不到你, 你仍可看到别人并主动发送; 隐身期间对方无法向你发起传输"
              checked={settings.passive}
              onChange={(v) => setSettings({ ...settings, passive: v })}
            />
            <ToggleRow
              label="开机自启"
              hint="登录后自动在后台启动(隐入托盘), 随时可接收文件"
              checked={settings.autostart}
              onChange={(v) => setSettings({ ...settings, autostart: v })}
            />

            {/* 吸底操作栏: 长内容滚动时保存/取消始终可见;
                毛玻璃模式下 bg-panel 是半透明变量, 必须叠 backdrop-blur
                把滚动上来的内容模糊掉, 否则文字会从按钮底下透出 */}
            <div className="sticky bottom-0 -mx-5 -mb-4 mt-5 flex items-center justify-end gap-3 border-t border-line bg-panel px-5 py-3 backdrop-blur-xl">
              {tip && <span className="max-w-52 truncate text-xs text-alert">{tip}</span>}
              <Button onClick={onClose}>取消</Button>
              <Button variant="primary" onClick={save}>
                保存
              </Button>
            </div>
          </>
        ) : (
          <div className="py-6 text-center text-xs text-mist">加载中…</div>
        )}
      </div>
    </ModalShell>
  );
}
