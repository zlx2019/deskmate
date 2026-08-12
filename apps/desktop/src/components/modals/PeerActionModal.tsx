// Peer actions for sending content, managing trust, and entering a pairing PIN.

import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { readText } from "@tauri-apps/plugin-clipboard-manager";
import { Card as ACard, Divider as ADivider, Tag as ATag } from "animal-island-ui";
import { api } from "../../api";
import { readClipboardImagePng } from "../../clipboard";
import { formatError, useI18n } from "../../i18n";
import { type PeerDto } from "../../types";
import { Avatar } from "../Radar";
import { Button, ModalShell, ToggleRow } from "./ModalShell";

/** Maps a platform ID to its display name for the tag. */
function platformLabel(platform: string): string {
  const map: Record<string, string> = { macos: "macOS", windows: "Windows", linux: "Linux" };
  return map[platform.toLowerCase()] ?? platform;
}

/** Peer dialog for sending files and text. */
export function PeerActionModal({
  peer,
  avatarSrc,
  getPin,
  onPinLearned,
  onSendFiles,
  onSendImage,
  onTextSent,
  onClose,
}: {
  peer: PeerDto;
  avatarSrc?: string;
  /** Session-cached peer PIN. */
  getPin: (fingerprint: string) => string | undefined;
  /** Stores a verified PIN in the session cache. */
  onPinLearned: (fingerprint: string, pin: string) => void;
  onSendFiles: (peer: PeerDto, paths: string[]) => void;
  /** Sends a clipboard screenshot through the file-transfer flow. */
  onSendImage: (peer: PeerDto, fileName: string, bytes: Uint8Array) => Promise<void>;
  /** Records successfully sent text in the message stream. */
  onTextSent: (peerName: string, text: string) => void;
  onClose: () => void;
}) {
  const { t } = useI18n();
  // Preserve text byte-for-byte without trimming.
  const [text, setText] = useState("");
  const [sending, setSending] = useState(false);
  const [sentTip, setSentTip] = useState<string | null>(null);
  // Expand PIN entry when the peer requests pairing.
  const [pinInput, setPinInput] = useState<string | null>(null);
  // Load trust state when opened and save immediately when toggled.
  const [trusted, setTrusted] = useState<boolean | null>(null);

  useEffect(() => {
    api
      .getSettings()
      .then((s) => setTrusted(s.trusted.some((t) => t.fingerprint === peer.fingerprint)))
      .catch(console.error);
  }, [peer.fingerprint]);

  /** Toggles trust by updating and immediately saving the latest allowlist. */
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
      setSentTip(formatError(e));
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

  /** Sends text and opens PIN entry when requested by the peer. */
  const deliver = async (content: string, okTip: string) => {
    setSending(true);
    try {
      // Prefer the newly entered PIN, then the session cache.
      const pin = pinInput?.trim() || getPin(peer.fingerprint);
      const { pinRequired } = await api.sendText(peer.fingerprint, content, pin);
      if (pinRequired) {
        // Wait for PIN entry when the peer requests pairing.
        setPinInput((prev) => prev ?? "");
        setSentTip(t.peer.pinRequired);
        return false;
      }
      if (pinInput?.trim()) onPinLearned(peer.fingerprint, pinInput.trim());
      setPinInput(null);
      onTextSent(peer.name, content);
      setSentTip(okTip);
      setTimeout(() => setSentTip(null), 1500);
      return true;
    } catch (e) {
      setSentTip(formatError(e));
      return false;
    } finally {
      setSending(false);
    }
  };

  const sendText = async () => {
    if (await deliver(text, t.peer.delivered)) setText("");
  };

  /** Sends clipboard content, preferring screenshots over text. */
  const sendClipboard = async () => {
    // The image branch also holds sending so the disabled button prevents repeats.
    setSending(true);
    try {
      const image = await readClipboardImagePng();
      if (image) {
        await onSendImage(peer, image.name, image.bytes);
        // Screenshots appear as file tasks in the right panel, so close the dialog.
        onClose();
        return;
      }
    } catch (e) {
      setSentTip(formatError(e));
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
        {/* Peer card with avatar, platform tag, and address. */}
        <ACard pattern="app-green" className="transfer-card">
          <div className="flex items-center gap-3">
            <div className="relative shrink-0">
              <Avatar
                name={peer.name}
                fingerprint={peer.fingerprint}
                size={48}
                avatar={peer.avatar}
                src={avatarSrc}
              />
              <span className="absolute -right-0.5 -bottom-0.5 size-2.5 rounded-full border-2 border-panel bg-live" />
            </div>
            <div className="min-w-0 flex-1">
              <div className="flex items-center gap-2">
                <span className="min-w-0 truncate text-sm font-bold text-fog">{peer.name}</span>
                <ATag size="small" variant="soft" color="app-teal">
                  <span title={peer.osVersion ?? undefined}>{platformLabel(peer.platform)}</span>
                </ATag>
              </div>
              <div className="mt-1 truncate font-gauge text-[11px] text-mist">
                {peer.addrs[0] ?? "?"}:{peer.port}
              </div>
            </div>
          </div>
        </ACard>

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

        <ADivider type="dashed-teal" className="my-3" />

        <div className="gauge-label mb-2">{t.peer.sendTextSection}</div>
        <textarea
          value={text}
          onChange={(e) => setText(e.target.value)}
          rows={4}
          placeholder={t.peer.textPlaceholder}
          className="w-full resize-none rounded-xl border-2 border-line bg-panel-2 px-3 py-2 font-gauge text-xs text-fog outline-none transition-colors placeholder:text-faint focus:border-sonar"
        />
        {/* Pairing PIN row shown when requested by the peer. */}
        {pinInput !== null && (
          <input
            autoFocus
            value={pinInput}
            onChange={(e) => setPinInput(e.target.value)}
            placeholder={t.peer.pinPlaceholder}
            className="mt-2 w-full rounded-xl border-2 border-ember/60 bg-panel-2 px-3 py-1.5 font-gauge text-sm text-fog outline-none transition-colors focus:border-ember"
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
