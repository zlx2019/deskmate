// 节点操作弹窗: 向选中节点发送文件/文件夹/文本/剪贴板, 并管理信任与配对 PIN 补填

import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { readText } from "@tauri-apps/plugin-clipboard-manager";
import { api } from "../../api";
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
  onClose,
}: {
  peer: PeerDto;
  avatarSrc?: string;
  /** 会话缓存的对端 PIN */
  getPin: (fingerprint: string) => string | undefined;
  /** PIN 验证通过后回写会话缓存 */
  onPinLearned: (fingerprint: string, pin: string) => void;
  onSendFiles: (peer: PeerDto, paths: string[]) => void;
  onClose: () => void;
}) {
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
        ? { directory: true, title: "选择要发送的文件夹" }
        : { multiple: true, title: "选择要发送的文件" },
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
        setSentTip("对方要求配对 PIN");
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
    if (await deliver(text, "已送达")) setText("");
  };

  /** 剪贴板一键发送: 读系统剪贴板文本直接送达(点击即明确意图, 不二次确认) */
  const sendClipboard = async () => {
    const clip = await readText().catch(() => null);
    if (!clip) {
      setSentTip("剪贴板没有文本");
      return;
    }
    await deliver(clip, "剪贴板已送达");
  };

  return (
    <ModalShell title="send to peer" onClose={onClose}>
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
            <div className="gauge-label mt-0.5">
              {peer.platform} · {peer.addrs[0] ?? "?"}:{peer.port}
            </div>
          </div>
        </div>

        <div className="mt-4 grid grid-cols-2 gap-2">
          <Button variant="primary" onClick={() => pickAndSend(false)}>
            发送文件…
          </Button>
          <Button onClick={() => pickAndSend(true)}>发送文件夹…</Button>
        </div>

        {trusted !== null && (
          <ToggleRow
            label="信任此设备"
            hint="它发来的文件将免确认自动接收到默认下载目录"
            checked={trusted}
            onChange={toggleTrust}
          />
        )}

        <div className="gauge-label mt-5 mb-2">send text</div>
        <textarea
          value={text}
          onChange={(e) => setText(e.target.value)}
          rows={4}
          placeholder="输入要发送的文本, 将逐字原样送达(保留空格与换行)"
          className="w-full resize-none rounded-md border border-line-2 bg-abyss/60 px-3 py-2 font-gauge text-xs text-fog outline-none transition-colors placeholder:text-mist/60 focus:border-sonar/60"
        />
        {/* 对方启用配对 PIN 时的补填行 */}
        {pinInput !== null && (
          <input
            autoFocus
            value={pinInput}
            onChange={(e) => setPinInput(e.target.value)}
            placeholder="输入对方的配对 PIN 后重新发送"
            className="mt-2 w-full rounded-md border border-ember/50 bg-abyss/60 px-3 py-1.5 font-gauge text-sm text-fog outline-none focus:border-ember"
          />
        )}
        <div className="mt-2 flex items-center justify-end gap-3">
          {sentTip && <span className="text-xs text-sonar">{sentTip}</span>}
          <Button disabled={sending} onClick={sendClipboard}>
            发送剪贴板
          </Button>
          <Button variant="primary" disabled={text.length === 0 || sending} onClick={sendText}>
            {sending ? "发送中…" : "发送文本"}
          </Button>
        </div>
      </div>
    </ModalShell>
  );
}
