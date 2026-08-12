// Persistent chat-style composer for sending text to a selected peer.
// It also handles the global send-clipboard hotkey and session PIN cache.

import { useEffect, useRef, useState } from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { readText } from "@tauri-apps/plugin-clipboard-manager";
import { Select as ASelect } from "animal-island-ui";
import { api } from "../api";
import { readClipboardImagePng, screenshotName } from "../clipboard";
import { EVENTS } from "../events";
import { formatError, getLocale, useI18n } from "../i18n";
import { type PeerDto } from "../types";

/** Composer with peer selection, text input, and optional PIN entry. */
export function MessageComposer({
  peers,
  getPin,
  onPinLearned,
  onSent,
  onImageSent,
  onSendImage,
  onSendFiles,
}: {
  peers: PeerDto[];
  /** Session-cached peer PIN. */
  getPin: (fingerprint: string) => string | undefined;
  /** Stores a verified PIN in the session cache. */
  onPinLearned: (fingerprint: string, pin: string) => void;
  /** Records a successfully sent message in the message stream. */
  onSent: (peerName: string, text: string) => void;
  /** Records a successfully sent clipboard image as an outgoing chat bubble. */
  onImageSent: (peerName: string, name: string, bytes: Uint8Array) => void;
  /** Sends a clipboard screenshot through the file-transfer flow. */
  onSendImage: (peer: PeerDto, fileName: string, bytes: Uint8Array) => Promise<void>;
  /** Sends files copied to the clipboard through the file-transfer flow. */
  onSendFiles: (peer: PeerDto, paths: string[]) => Promise<void>;
}) {
  const { t } = useI18n();
  // Preserve text byte-for-byte without trimming.
  const [text, setText] = useState("");
  const [targetFp, setTargetFp] = useState("");
  const [sending, setSending] = useState(false);
  // Color the status bar differently for errors and neutral progress notices.
  const [tip, setTip] = useState<{ text: string; error: boolean } | null>(null);
  // Expand the PIN row when the peer requests pairing.
  const [pinInput, setPinInput] = useState<string | null>(null);

  // Fall back to the first peer when the selected device goes offline.
  const target = peers.find((p) => p.fingerprint === targetFp) ?? peers[0];

  /** Sends text from manual input or clipboard and expands PIN entry when required. */
  const deliver = async (content: string): Promise<boolean> => {
    if (!target || content.length === 0 || sending) return false;
    setSending(true);
    try {
      // Prefer the newly entered PIN, then the session cache.
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
      setTip({ text: formatError(e), error: true });
      return false;
    } finally {
      setSending(false);
    }
  };

  /** Sends the input text and clears it after success. */
  const send = async () => {
    if (await deliver(text)) setText("");
  };

  // The global hotkey sends clipboard content to the selected peer, preferring images.
  const deliverRef = useRef(deliver);
  deliverRef.current = deliver;
  const targetRef = useRef(target);
  targetRef.current = target;
  const onSendImageRef = useRef(onSendImage);
  onSendImageRef.current = onSendImage;
  const onImageSentRef = useRef(onImageSent);
  onImageSentRef.current = onImageSent;
  // A ref prevents duplicate screenshot sends before React can re-render.
  const imageBusyRef = useRef(false);

  /** Sends a screenshot from hotkey or paste with reentry protection and feedback.
   * The persistent hotkey closure resolves current text through getLocale. */
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
      // Mirror the outgoing image into the chat stream like sent text.
      onImageSentRef.current(peer.name, fileName, bytes);
      if (notifyResult) {
        api.notify(msg.notifyScreenshotSending, msg.notifyTo(peer.name)).catch(console.error);
      } else {
        setTip({ text: msg.screenshotSendingTip, error: false });
        setTimeout(() => setTip(null), 2000);
      }
    } catch (e) {
      if (notifyResult) {
        api.notify(msg.notifyScreenshotFailed, formatError(e)).catch(console.error);
      } else {
        setTip({ text: formatError(e), error: true });
      }
    } finally {
      imageBusyRef.current = false;
    }
  };
  const sendImageRef = useRef(sendImage);
  sendImageRef.current = sendImage;

  const onSendFilesRef = useRef(onSendFiles);
  onSendFilesRef.current = onSendFiles;
  // File-send reentry is guarded independently from screenshot sends.
  const filesBusyRef = useRef(false);

  /** Sends copied clipboard files with reentry protection and notification feedback. */
  const sendClipFiles = async (peer: PeerDto, paths: string[]) => {
    if (filesBusyRef.current) return;
    filesBusyRef.current = true;
    const msg = getLocale().composer;
    try {
      await onSendFilesRef.current(peer, paths);
      api.notify(msg.notifyFilesSending(paths.length), msg.notifyTo(peer.name)).catch(console.error);
    } catch (e) {
      api.notify(msg.notifyFilesFailed, formatError(e)).catch(console.error);
    } finally {
      filesBusyRef.current = false;
    }
  };
  const sendClipFilesRef = useRef(sendClipFiles);
  sendClipFilesRef.current = sendClipFiles;

  useEffect(() => {
    if (!("__TAURI_INTERNALS__" in window)) return;
    let alive = true;
    let unlisten: UnlistenFn | undefined;
    listen(EVENTS.HOTKEY_SEND_CLIPBOARD, async () => {
      // The one-time listener closure resolves current text through getLocale.
      const msg = getLocale().composer;
      const peer = targetRef.current;
      if (!peer) {
        api.notify("Deskmate", msg.notifyNoPeer).catch(console.error);
        return;
      }
      // Send copied files before bitmap content so copied image files retain
      // their original data instead of being re-encoded as screenshots.
      const files = await api.readClipboardFiles().catch(() => [] as string[]);
      if (files.length > 0) {
        await sendClipFilesRef.current(peer, files);
        return;
      }
      // Encode clipboard screenshots as PNG and send them as normal files.
      const image = await readClipboardImagePng();
      if (image) {
        await sendImageRef.current(peer, image.name, image.bytes, true);
        return;
      }
      const clip = await readText().catch(() => null);
      if (!clip) {
        api.notify("Deskmate", msg.notifyNoClip).catch(console.error);
        return;
      }
      if (await deliverRef.current(clip)) {
        api.notify(msg.notifyClipSent, msg.notifyTo(peer.name)).catch(console.error);
      } else {
        api.notify(msg.notifyClipFailed, msg.notifyOpenApp).catch(console.error);
      }
    }).then((u) => {
      // Immediately remove a late subscription from StrictMode's duplicate effect.
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
        <div className="composer-select min-w-0 flex-1">
          <ASelect
            options={peers.map((p) => ({ key: p.fingerprint, label: p.name }))}
            value={target?.fingerprint ?? ""}
            onChange={setTargetFp}
            aria-label={t.composer.to}
          />
        </div>
      </div>
      {/* Keep long status text on its own row so it cannot crowd peer selection. */}
      {tip && (
        <div
          className={`mt-2 rounded-xl border-2 px-2.5 py-1.5 text-[11px] leading-relaxed break-all ${
            tip.error
              ? "border-alert/40 bg-alert/10 text-alert"
              : "border-line bg-panel-2 text-mist"
          }`}
        >
          {tip.text}
        </div>
      )}
      {/* Pairing PIN row shown when requested by the peer. */}
      {pinInput !== null && (
        <input
          autoFocus
          value={pinInput}
          onChange={(e) => setPinInput(e.target.value)}
          placeholder={t.composer.pinPlaceholder}
          className="mt-2 w-full rounded-xl border-2 border-ember/60 bg-panel-2 px-3 py-1.5 font-gauge text-sm text-fog outline-none transition-colors focus:border-ember"
        />
      )}
      <div className="mt-2 flex items-end gap-2">
        <textarea
          value={text}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            // Enter sends, Shift+Enter adds a line, and IME composition does neither.
            if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
              e.preventDefault();
              void send();
            }
          }}
          onPaste={(e) => {
            // Send pasted images as screenshot files because textarea cannot contain them.
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
          className="min-w-0 flex-1 resize-none rounded-xl border-2 border-line bg-panel-2 px-2.5 py-1.5 font-gauge text-xs text-fog outline-none transition-colors placeholder:text-faint focus:border-sonar"
        />
        <button
          onClick={() => void send()}
          disabled={text.length === 0 || sending}
          title={t.composer.send}
          className="flex size-8 shrink-0 cursor-pointer items-center justify-center rounded-full bg-sonar text-white shadow-[0_2px_0_rgba(41,71,51,0.2)] transition-colors hover:bg-sonar-dim disabled:cursor-default disabled:bg-line-2 disabled:text-white/70 disabled:shadow-none"
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
