// Deskmate main view: radar, transfer panel, dialogs, and global file drag-and-drop.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { EVENTS } from "./events";
import { useDeskmate } from "./hooks/useDeskmate";
import { useI18n } from "./i18n";
import { Radar } from "./components/Radar";
import { TransferPanel } from "./components/TransferPanel";
import {
  OfferModal,
  PeerActionModal,
  PinModal,
  SettingsModal,
} from "./components/modals";
import { api } from "./api";
import { avatarHashOf, type PeerDto, type TransferItem } from "./types";

/** Maps drag-event physical coordinates to CSS coordinates and a peer fingerprint. */
function hitPeer(pos: { x: number; y: number }): string | null {
  const scale = window.devicePixelRatio || 1;
  const el = document.elementFromPoint(pos.x / scale, pos.y / scale);
  return el?.closest?.("[data-peer]")?.getAttribute("data-peer") ?? null;
}

/** Application root component. */
export default function App() {
  const { t } = useI18n();
  const dm = useDeskmate();
  const [activePeer, setActivePeer] = useState<PeerDto | null>(null);
  const [showSettings, setShowSettings] = useState(false);
  // Rejected task awaiting a PIN retry.
  const [pinRetry, setPinRetry] = useState<TransferItem | null>(null);
  // Native window chrome follows the interface style through theme.ts.
  const [dragging, setDragging] = useState(false);
  const [dragHover, setDragHover] = useState<string | null>(null);

  // Refs provide current peers and sendFiles without re-registering drag handlers.
  const peersRef = useRef(dm.peers);
  peersRef.current = dm.peers;
  const sendFilesRef = useRef(dm.sendFiles);
  sendFilesRef.current = dm.sendFiles;

  // Global file drag highlights the target peer and sends on drop.
  useEffect(() => {
    let alive = true;
    let unlisten: (() => void) | undefined;
    // Browser-only previews have no Tauri drag subscription.
    if (!("__TAURI_INTERNALS__" in window)) return;
    getCurrentWebview()
      .onDragDropEvent((event) => {
        const payload = event.payload;
        if (payload.type === "enter" || payload.type === "over") {
          setDragging(true);
          setDragHover(hitPeer(payload.position));
        } else if (payload.type === "drop") {
          const fp = hitPeer(payload.position);
          setDragging(false);
          setDragHover(null);
          const peer = fp ? peersRef.current[fp] : undefined;
          if (peer && payload.paths.length > 0) {
            sendFilesRef.current(peer, payload.paths).catch(console.error);
          }
        } else {
          setDragging(false);
          setDragHover(null);
        }
      })
      .then((u) => {
        if (alive) unlisten = u;
        else u();
      });
    return () => {
      alive = false;
      unlisten?.();
    };
  }, []);

  // The tray settings event opens the dialog after the backend shows the window.
  useEffect(() => {
    if (!("__TAURI_INTERNALS__" in window)) return;
    let alive = true;
    let unlisten: (() => void) | undefined;
    listen(EVENTS.OPEN_SETTINGS, () => setShowSettings(true)).then((u) => {
      if (alive) unlisten = u;
      else u();
    });
    return () => {
      alive = false;
      unlisten?.();
    };
  }, []);

  // Global shortcuts: Escape closes the top dialog, mod+, opens settings, and
  // mod+W hides the window. Incoming offers require an explicit decision.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const mod = e.metaKey || e.ctrlKey;
      if (e.key === "Escape") {
        if (showSettings) setShowSettings(false);
        else if (pinRetry) setPinRetry(null);
        else if (activePeer) setActivePeer(null);
      } else if (mod && e.key === ",") {
        e.preventDefault();
        setShowSettings(true);
      } else if (mod && e.key.toLowerCase() === "w") {
        // macOS Cmd+W uses the intercepted system close action. Supply the same
        // hide-to-tray behavior explicitly on Windows and Linux.
        e.preventDefault();
        if ("__TAURI_INTERNALS__" in window) {
          getCurrentWindow().hide().catch(console.error);
        }
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [showSettings, pinRetry, activePeer]);

  const self = dm.self;
  // Stable derived values let memoized children avoid high-frequency transfer renders.
  const peerList = useMemo(() => Object.values(dm.peers), [dm.peers]);
  const transferList = useMemo(() => Object.values(dm.transfers), [dm.transfers]);
  const openSettings = useCallback(() => setShowSettings(true), []);
  /** Returns an avatar image URL, or undefined when unavailable. */
  const srcOf = (avatar: string | null | undefined) => {
    const hash = avatarHashOf(avatar);
    return hash ? dm.avatarSrcs[hash] : undefined;
  };

  return (
    <div className="flex h-full flex-col">
      <div className="flex min-h-0 flex-1">
        <main className="relative min-w-0 flex-1">
          <Radar
            self={self}
            peers={peerList}
            avatarSrcs={dm.avatarSrcs}
            dragging={dragging}
            dragHover={dragHover}
            onPeerClick={setActivePeer}
          />
          {/* Drag guidance above the persistent scanner without blocking hit testing. */}
          {dragging && (
            <div className="pointer-events-none absolute inset-x-0 bottom-20 text-center">
              <span className="rounded-full border border-ember/60 bg-panel px-4 py-1.5 text-sm text-ember">
                {dragHover ? t.radar.dropToSend : t.radar.dragToTarget}
              </span>
            </div>
          )}
        </main>
        {/* The panel narrows with the window and remains 320px at normal sizes. */}
        <aside className="w-[clamp(17.5rem,36vw,20rem)] shrink-0 border-l border-line bg-panel transition-colors duration-300">
          <TransferPanel
            transfers={transferList}
            texts={dm.texts}
            peers={peerList}
            getPin={dm.getPin}
            onPause={dm.pauseTransfer}
            onResume={dm.resumeTransfer}
            onPinLearned={dm.rememberPin}
            onTextSent={dm.addSentText}
            onImageSent={dm.addSentImage}
            onSendImage={dm.sendClipboardImage}
            onSendFiles={dm.sendFiles}
            onRemoveText={dm.removeText}
            onClearTexts={dm.clearTexts}
            onPinRetry={setPinRetry}
            onOpenSettings={openSettings}
          />
        </aside>
      </div>

      {/* Incoming-offer dialogs are queued and shown one at a time. The offerId
          key forces remounting so one offer's local choices cannot leak into the next. */}
      {dm.offers[0] && (
        <OfferModal
          key={dm.offers[0].offerId}
          offer={dm.offers[0]}
          avatarSrc={srcOf(dm.offers[0].peerAvatar)}
          onRespond={dm.respondOffer}
        />
      )}
      {activePeer && (
        <PeerActionModal
          peer={activePeer}
          avatarSrc={srcOf(activePeer.avatar)}
          getPin={dm.getPin}
          onPinLearned={dm.rememberPin}
          onSendFiles={(peer, paths) => {
            dm.sendFiles(peer, paths).catch(console.error);
          }}
          onSendImage={dm.sendClipboardImage}
          onTextSent={dm.addSentText}
          onImageSent={dm.addSentImage}
          onClose={() => setActivePeer(null)}
        />
      )}
      {pinRetry && (
        <PinModal
          peerName={pinRetry.peerName}
          onSubmit={(pin) => {
            api.retrySend(pinRetry.transferId, pin).catch(console.error);
            // Later events update retry status; cache the PIN before they arrive.
            if (pinRetry.peerFingerprint) dm.rememberPin(pinRetry.peerFingerprint, pin);
            setPinRetry(null);
          }}
          onClose={() => setPinRetry(null)}
        />
      )}
      {showSettings && self && (
        <SettingsModal
          fingerprint={self.fingerprint}
          peers={peerList}
          avatarSrcs={dm.avatarSrcs}
          // Refresh local-device display after live profile and directory updates.
          onSaved={dm.refreshSelf}
          onClose={() => setShowSettings(false)}
        />
      )}
    </div>
  );
}
