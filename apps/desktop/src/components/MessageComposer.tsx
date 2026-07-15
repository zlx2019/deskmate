// 聊天式消息输入行: 选目标设备直接发文本(右栏文字消息区底部常驻)

import { useState } from "react";
import { api } from "../api";
import { type PeerDto } from "../types";

/** 聊天输入行: 设备下拉 + 文本框(Enter 发送 / Shift+Enter 换行)+ PIN 补填 */
export function MessageComposer({
  peers,
  getPin,
  onPinLearned,
  onSent,
}: {
  peers: PeerDto[];
  /** 会话缓存的对端 PIN */
  getPin: (fingerprint: string) => string | undefined;
  /** PIN 验证通过后回写会话缓存 */
  onPinLearned: (fingerprint: string, pin: string) => void;
  /** 发送成功后回调(把消息记入消息流) */
  onSent: (peerName: string, text: string) => void;
}) {
  // 文本必须逐字节原样发送, 不做任何 trim
  const [text, setText] = useState("");
  const [targetFp, setTargetFp] = useState("");
  const [sending, setSending] = useState(false);
  const [tip, setTip] = useState<string | null>(null);
  // 对方要求配对 PIN 时展开补填行
  const [pinInput, setPinInput] = useState<string | null>(null);

  // 选中的设备下线后自动回退到列表首位
  const target = peers.find((p) => p.fingerprint === targetFp) ?? peers[0];

  /** 发送当前文本; 对方要求 PIN 时展开输入行等待补填后重发 */
  const send = async () => {
    if (!target || text.length === 0 || sending) return;
    setSending(true);
    try {
      // 优先带刚补填的 PIN, 其次是会话缓存
      const pin = pinInput?.trim() || getPin(target.fingerprint);
      const { pinRequired } = await api.sendText(target.fingerprint, text, pin);
      if (pinRequired) {
        setPinInput((prev) => prev ?? "");
        setTip("对方要求配对 PIN");
        return;
      }
      if (pinInput?.trim()) onPinLearned(target.fingerprint, pinInput.trim());
      setPinInput(null);
      setTip(null);
      onSent(target.name, text);
      setText("");
    } catch (e) {
      setTip(String(e));
    } finally {
      setSending(false);
    }
  };

  if (peers.length === 0) {
    return (
      <div className="border-t border-line px-4 py-3 text-center text-xs text-mist/70">
        暂无在线设备, 无法发送消息
      </div>
    );
  }

  return (
    <div className="border-t border-line px-3 py-2.5">
      <div className="flex items-center gap-2">
        <span className="shrink-0 text-[11px] text-mist">发给</span>
        <select
          value={target?.fingerprint ?? ""}
          onChange={(e) => setTargetFp(e.target.value)}
          className="min-w-0 flex-1 cursor-pointer appearance-none rounded border border-line-2 bg-abyss/60 px-2 py-1 text-xs text-fog outline-none transition-colors focus:border-sonar/60"
        >
          {peers.map((p) => (
            <option key={p.fingerprint} value={p.fingerprint}>
              {p.name}
            </option>
          ))}
        </select>
        {tip && <span className="shrink-0 text-[11px] text-ember">{tip}</span>}
      </div>
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
      <div className="mt-2 flex items-end gap-2">
        <textarea
          value={text}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            // Enter 发送, Shift+Enter 换行; 输入法组词中的 Enter 不触发
            if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
              e.preventDefault();
              void send();
            }
          }}
          rows={2}
          placeholder="输入消息, Enter 发送(逐字原样送达)"
          className="min-w-0 flex-1 resize-none rounded-md border border-line-2 bg-abyss/60 px-2.5 py-1.5 font-gauge text-xs text-fog outline-none transition-colors placeholder:text-mist/60 focus:border-sonar/60"
        />
        <button
          onClick={() => void send()}
          disabled={text.length === 0 || sending}
          title="发送"
          className="flex size-8 shrink-0 cursor-pointer items-center justify-center rounded-md border border-sonar/50 text-sonar transition-colors hover:bg-sonar/10 disabled:cursor-default disabled:border-line-2 disabled:text-mist/50"
        >
          {sending ? (
            <span className="text-[11px]">…</span>
          ) : (
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
              <path d="M22 2 11 13" />
              <path d="M22 2 15 22l-4-9-9-4z" />
            </svg>
          )}
        </button>
      </div>
    </div>
  );
}
