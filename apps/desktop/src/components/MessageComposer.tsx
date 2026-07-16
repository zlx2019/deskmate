// 聊天式消息输入行: 选目标设备直接发文本(右栏文字消息区底部常驻)
// 全局快捷键"发送剪贴板"也在此消费: 目标选择与 PIN 会话缓存都在这里

import { useEffect, useRef, useState } from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { readText } from "@tauri-apps/plugin-clipboard-manager";
import { api } from "../api";
import { readClipboardImagePng, screenshotName } from "../clipboard";
import { EVENTS } from "../events";
import { getLocale, useI18n } from "../i18n";
import { type PeerDto } from "../types";

/** 聊天输入行: 设备下拉 + 文本框(Enter 发送 / Shift+Enter 换行)+ PIN 补填 */
export function MessageComposer({
  peers,
  getPin,
  onPinLearned,
  onSent,
  onSendImage,
}: {
  peers: PeerDto[];
  /** 会话缓存的对端 PIN */
  getPin: (fingerprint: string) => string | undefined;
  /** PIN 验证通过后回写会话缓存 */
  onPinLearned: (fingerprint: string, pin: string) => void;
  /** 发送成功后回调(把消息记入消息流) */
  onSent: (peerName: string, text: string) => void;
  /** 发送剪贴板截图(走文件传输链, 由快捷键触发) */
  onSendImage: (peer: PeerDto, fileName: string, bytes: Uint8Array) => Promise<void>;
}) {
  const { t } = useI18n();
  // 文本必须逐字节原样发送, 不做任何 trim
  const [text, setText] = useState("");
  const [targetFp, setTargetFp] = useState("");
  const [sending, setSending] = useState(false);
  // 提示条: 错误(发送失败/需要 PIN)与中性提示(截图发送中)分色呈现
  const [tip, setTip] = useState<{ text: string; error: boolean } | null>(null);
  // 对方要求配对 PIN 时展开补填行
  const [pinInput, setPinInput] = useState<string | null>(null);

  // 选中的设备下线后自动回退到列表首位
  const target = peers.find((p) => p.fingerprint === targetFp) ?? peers[0];

  /** 发送指定内容(手动输入与快捷键剪贴板共用); 对方要求 PIN 时展开补填行 */
  const deliver = async (content: string): Promise<boolean> => {
    if (!target || content.length === 0 || sending) return false;
    setSending(true);
    try {
      // 优先带刚补填的 PIN, 其次是会话缓存
      const pin = pinInput?.trim() || getPin(target.fingerprint);
      const { pinRequired } = await api.sendText(target.fingerprint, content, pin);
      if (pinRequired) {
        setPinInput((prev) => prev ?? "");
        setTip({ text: getLocale().composer.pinRequired, error: true });
        return false;
      }
      if (pinInput?.trim()) onPinLearned(target.fingerprint, pinInput.trim());
      setPinInput(null);
      setTip(null);
      onSent(target.name, content);
      return true;
    } catch (e) {
      setTip({ text: String(e), error: true });
      return false;
    } finally {
      setSending(false);
    }
  };

  /** 发送输入框文本, 成功后清空 */
  const send = async () => {
    if (await deliver(text)) setText("");
  };

  // 全局快捷键: 读剪贴板发给当前选中设备, 截图优先于文本(ref 透传避免闭包过期)
  const deliverRef = useRef(deliver);
  deliverRef.current = deliver;
  const targetRef = useRef(target);
  targetRef.current = target;
  const onSendImageRef = useRef(onSendImage);
  onSendImageRef.current = onSendImage;
  // 截图发送防重入(快捷键连按/粘贴连击); ref 即时生效, 不等重渲染
  const imageBusyRef = useRef(false);

  /** 发送一张截图(快捷键与粘贴共用): 防重入 + 结果反馈
   * (快捷键路径的闭包挂载后不更新, 文案经 getLocale 取当前语言) */
  const sendImage = async (
    peer: PeerDto,
    fileName: string,
    bytes: Uint8Array,
    notifyResult: boolean,
  ) => {
    if (imageBusyRef.current) return;
    imageBusyRef.current = true;
    const msg = getLocale().composer;
    try {
      await onSendImageRef.current(peer, fileName, bytes);
      if (notifyResult) {
        api.notify(msg.notifyScreenshotSending, msg.notifyTo(peer.name)).catch(console.error);
      } else {
        setTip({ text: msg.screenshotSendingTip, error: false });
        setTimeout(() => setTip(null), 2000);
      }
    } catch (e) {
      if (notifyResult) {
        api.notify(msg.notifyScreenshotFailed, String(e)).catch(console.error);
      } else {
        setTip({ text: String(e), error: true });
      }
    } finally {
      imageBusyRef.current = false;
    }
  };
  const sendImageRef = useRef(sendImage);
  sendImageRef.current = sendImage;
  useEffect(() => {
    if (!("__TAURI_INTERNALS__" in window)) return;
    let alive = true;
    let unlisten: UnlistenFn | undefined;
    listen(EVENTS.HOTKEY_SEND_CLIPBOARD, async () => {
      // 一次性挂载的监听闭包: 文案经 getLocale 取当前语言, 不随组件重渲
      const msg = getLocale().composer;
      const peer = targetRef.current;
      if (!peer) {
        api.notify("deskmate", msg.notifyNoPeer).catch(console.error);
        return;
      }
      // 剪贴板是截图: 编码 PNG 走文件传输链(对端按普通文件接收)
      const image = await readClipboardImagePng();
      if (image) {
        await sendImageRef.current(peer, image.name, image.bytes, true);
        return;
      }
      const clip = await readText().catch(() => null);
      if (!clip) {
        api.notify("deskmate", msg.notifyNoClip).catch(console.error);
        return;
      }
      if (await deliverRef.current(clip)) {
        api.notify(msg.notifyClipSent, msg.notifyTo(peer.name)).catch(console.error);
      } else {
        api.notify(msg.notifyClipFailed, msg.notifyOpenApp).catch(console.error);
      }
    }).then((u) => {
      // StrictMode 下 effect 双跑, 迟到的订阅立即退订
      if (alive) unlisten = u;
      else u();
    });
    return () => {
      alive = false;
      unlisten?.();
    };
  }, []);

  if (peers.length === 0) {
    return (
      <div className="border-t border-line px-4 py-3 text-center text-xs text-mist/70">
        {t.composer.noPeers}
      </div>
    );
  }

  return (
    <div className="border-t border-line px-3 py-2.5">
      <div className="flex items-center gap-2">
        <span className="shrink-0 text-[11px] text-mist">{t.composer.to}</span>
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
      </div>
      {/* 提示条独立成行: 错误串可能很长(含地址/原因), 行内展示会挤爆设备选择行 */}
      {tip && (
        <div
          className={`mt-2 rounded-md border px-2.5 py-1.5 text-[11px] leading-relaxed break-all ${
            tip.error
              ? "border-alert/30 bg-alert/10 text-alert"
              : "border-line-2 bg-abyss/40 text-mist"
          }`}
        >
          {tip.text}
        </div>
      )}
      {/* 对方启用配对 PIN 时的补填行 */}
      {pinInput !== null && (
        <input
          autoFocus
          value={pinInput}
          onChange={(e) => setPinInput(e.target.value)}
          placeholder={t.composer.pinPlaceholder}
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
          onPaste={(e) => {
            // 粘贴的是图片(截图): 直接按截图文件发送, textarea 承载不了图片
            const item = Array.from(e.clipboardData?.items ?? []).find((i) =>
              i.type.startsWith("image/"),
            );
            const file = item?.getAsFile();
            if (!file || !target) return;
            e.preventDefault();
            void (async () => {
              const bytes = new Uint8Array(await file.arrayBuffer());
              await sendImage(target, screenshotName(), bytes, false);
            })();
          }}
          rows={2}
          placeholder={t.composer.placeholder}
          className="min-w-0 flex-1 resize-none rounded-md border border-line-2 bg-abyss/60 px-2.5 py-1.5 font-gauge text-xs text-fog outline-none transition-colors placeholder:text-mist/60 focus:border-sonar/60"
        />
        <button
          onClick={() => void send()}
          disabled={text.length === 0 || sending}
          title={t.composer.send}
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
