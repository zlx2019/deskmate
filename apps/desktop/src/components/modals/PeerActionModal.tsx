// 节点操作弹窗: 向选中节点发送文件/文件夹/文本/剪贴板, 并管理信任与配对 PIN 补填

import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { readText } from "@tauri-apps/plugin-clipboard-manager";
import { api } from "../../api";
import { readClipboardImagePng } from "../../clipboard";
import { useI18n } from "../../i18n";
import { type PeerDto } from "../../types";
import { Avatar } from "../Radar";
import { Button, ModalShell, ToggleRow } from "./ModalShell";

/** 节点操作弹窗: 发送文件 / 发送文本 */
export function PeerActionModal({
  peer,
  avatarSrc,
  getPin,
  onPinLearned,
  onSendFiles,
  onSendImage,
  onClose,
}: {
  peer: PeerDto;
  avatarSrc?: string;
  /** 会话缓存的对端 PIN */
  getPin: (fingerprint: string) => string | undefined;
  /** PIN 验证通过后回写会话缓存 */
  onPinLearned: (fingerprint: string, pin: string) => void;
  onSendFiles: (peer: PeerDto, paths: string[]) => void;
  /** 发送剪贴板截图(走文件传输链) */
  onSendImage: (peer: PeerDto, fileName: string, bytes: Uint8Array) => Promise<void>;
  onClose: () => void;
}) {
  const { t } = useI18n();
  // 文本内容必须逐字节原样发送, 不做任何 trim
  const [text, setText] = useState("");
  const [sending, setSending] = useState(false);
  const [sentTip, setSentTip] = useState<string | null>(null);
  // 对方要求配对 PIN 时展开输入行
  const [pinInput, setPinInput] = useState<string | null>(null);
  // 信任状态: 打开弹窗时从设置读取, 切换即保存
  const [trusted, setTrusted] = useState<boolean | null>(null);

  useEffect(() => {
    api
      .getSettings()
      .then((s) => setTrusted(s.trusted.some((t) => t.fingerprint === peer.fingerprint)))
      .catch(console.error);
  }, [peer.fingerprint]);

  /** 切换信任: 读取最新设置改写白名单后立即保存 */
  const toggleTrust = async (next: boolean) => {
    try {
      const s = await api.getSettings();
      const rest = s.trusted.filter((t) => t.fingerprint !== peer.fingerprint);
      const trustedList = next
        ? [...rest, { fingerprint: peer.fingerprint, name: peer.name }]
        : rest;
      await api.saveSettings({ ...s, trusted: trustedList });
      setTrusted(next);
    } catch (e) {
      setSentTip(String(e));
    }
  };

  const pickAndSend = async (directory: boolean) => {
    const picked = await open(
      directory
        ? { directory: true, title: t.peer.pickFolderTitle }
        : { multiple: true, title: t.peer.pickFilesTitle },
    );
    if (!picked) return;
    const paths = Array.isArray(picked) ? picked : [picked];
    if (paths.length > 0) {
      onSendFiles(peer, paths);
      onClose();
    }
  };

  /** 发送一段文本; 对方要求 PIN 时展开输入行等待补填 */
  const deliver = async (content: string, okTip: string) => {
    setSending(true);
    try {
      // 优先带弹窗内刚输入的 PIN, 其次是会话缓存
      const pin = pinInput?.trim() || getPin(peer.fingerprint);
      const { pinRequired } = await api.sendText(peer.fingerprint, content, pin);
      if (pinRequired) {
        // 对方要求配对 PIN: 展开输入行等待补填
        setPinInput((prev) => prev ?? "");
        setSentTip(t.peer.pinRequired);
        return false;
      }
      if (pinInput?.trim()) onPinLearned(peer.fingerprint, pinInput.trim());
      setPinInput(null);
      setSentTip(okTip);
      setTimeout(() => setSentTip(null), 1500);
      return true;
    } catch (e) {
      setSentTip(String(e));
      return false;
    } finally {
      setSending(false);
    }
  };

  const sendText = async () => {
    if (await deliver(text, t.peer.delivered)) setText("");
  };

  /** 剪贴板一键发送: 截图优先走文件传输链, 否则按文本直接送达 */
  const sendClipboard = async () => {
    // 图片分支同样占用 sending, 让按钮 disabled 生效防连击
    setSending(true);
    try {
      const image = await readClipboardImagePng();
      if (image) {
        await onSendImage(peer, image.name, image.bytes);
        // 截图按文件任务发送, 进度在右栏; 关弹窗让用户看到任务条目
        onClose();
        return;
      }
    } catch (e) {
      setSentTip(String(e));
      return;
    } finally {
      setSending(false);
    }
    const clip = await readText().catch(() => null);
    if (!clip) {
      setSentTip(t.peer.noClip);
      return;
    }
    await deliver(clip, t.peer.clipDelivered);
  };

  return (
    <ModalShell title={t.peer.title} onClose={onClose}>
      <div className="px-5 py-4">
        <div className="flex items-center gap-3">
          <Avatar
            name={peer.name}
            fingerprint={peer.fingerprint}
            size={44}
            avatar={peer.avatar}
            src={avatarSrc}
          />
          <div className="min-w-0">
            <div className="truncate text-sm font-medium text-fog">{peer.name}</div>
            <div className="gauge-label mt-0.5">OS: {peer.osVersion ?? peer.platform}</div>
            <div className="gauge-label mt-0.5">
              IP: {peer.addrs[0] ?? "?"}:{peer.port}
            </div>
          </div>
        </div>

        <div className="mt-4 grid grid-cols-2 gap-2">
          <Button variant="primary" onClick={() => pickAndSend(false)}>
            {t.peer.sendFiles}
          </Button>
          <Button onClick={() => pickAndSend(true)}>{t.peer.sendFolder}</Button>
        </div>

        {trusted !== null && (
          <ToggleRow
            label={t.peer.trustLabel}
            hint={t.peer.trustHint}
            checked={trusted}
            onChange={toggleTrust}
          />
        )}

        <div className="gauge-label mt-5 mb-2">{t.peer.sendTextSection}</div>
        <textarea
          value={text}
          onChange={(e) => setText(e.target.value)}
          rows={4}
          placeholder={t.peer.textPlaceholder}
          className="w-full resize-none rounded-md border border-line-2 bg-abyss/60 px-3 py-2 font-gauge text-xs text-fog outline-none transition-colors placeholder:text-mist/60 focus:border-sonar/60"
        />
        {/* 对方启用配对 PIN 时的补填行 */}
        {pinInput !== null && (
          <input
            autoFocus
            value={pinInput}
            onChange={(e) => setPinInput(e.target.value)}
            placeholder={t.peer.pinPlaceholder}
            className="mt-2 w-full rounded-md border border-ember/50 bg-abyss/60 px-3 py-1.5 font-gauge text-sm text-fog outline-none focus:border-ember"
          />
        )}
        <div className="mt-2 flex items-center justify-end gap-3">
          {sentTip && <span className="text-xs text-sonar">{sentTip}</span>}
          <Button disabled={sending} onClick={sendClipboard}>
            {t.peer.sendClipboard}
          </Button>
          <Button variant="primary" disabled={text.length === 0 || sending} onClick={sendText}>
            {sending ? t.peer.sending : t.peer.sendText}
          </Button>
        </div>
      </div>
    </ModalShell>
  );
}
