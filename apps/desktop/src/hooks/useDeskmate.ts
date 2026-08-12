// Deskmate frontend state core: subscribes to engine events and aggregates app state.

import { useCallback, useEffect, useMemo, useReducer, useRef, useState } from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { Notification as ANotification } from "animal-island-ui";
import { api } from "../api";
import { EVENTS } from "../events";
import { formatErrorCode, getLocale } from "../i18n";
import {
  avatarBlobUrl,
  avatarHashOf,
  type OfferDto,
  type PeerDto,
  type SelfInfoDto,
  type TextMsg,
  type TransferEventDto,
  type TransferItem,
} from "../types";

/** Speed samples keyed by transfer ID and scoped to one component lifecycle. */
type SpeedSamples = Map<string, { done: number; at: number; file: string }>;

type TransferAction =
  | {
      type: "begin";
      transferId: string;
      direction: "send" | "recv";
      peerName: string;
      peerFingerprint: string;
    }
  | { type: "event"; event: Exclude<TransferEventDto, { kind: "textReceived" }>; at: number }
  // Local pause and resume state; peer state arrives through engine events.
  | { type: "setPaused"; transferId: string; paused: boolean };

/** Derives active state from local and peer pause flags. */
function runningStatus(pausedLocal?: boolean, pausedByPeer?: boolean): "active" | "paused" {
  return pausedLocal || pausedByPeer ? "paused" : "active";
}

/** Builds a reducer that folds per-file engine events into panel entries.
 *
 * The injected speed-sample map follows the component lifecycle instead of
 * retaining unfinished task data at module scope. */
function makeTransferReducer(speedSamples: SpeedSamples) {
  return function transferReducer(
    state: Record<string, TransferItem>,
    action: TransferAction,
  ): Record<string, TransferItem> {
  if (action.type === "begin") {
    const { transferId, direction, peerName, peerFingerprint } = action;
    return {
      ...state,
      [transferId]: {
        transferId,
        direction,
        peerName,
        peerFingerprint,
        status: "active",
        currentFile: getLocale().transfer.waitingResponse,
        done: 0,
        size: 0,
        filesDone: 0,
        speed: 0,
        startedAt: Date.now(),
      },
    };
  }

  // Local pause and resume affect only active tasks. Both local and peer flags
  // must clear before state returns to active.
  if (action.type === "setPaused") {
    const item = state[action.transferId];
    if (!item || (item.status !== "active" && item.status !== "paused")) return state;
    const status = runningStatus(action.paused, item.pausedByPeer);
    return {
      ...state,
      [action.transferId]: {
        ...item,
        pausedLocal: action.paused,
        status,
        speed: status === "paused" ? 0 : item.speed,
      },
    };
  }

  const ev = action.event;
  // Defensive fallback for an unknown task when begin should have arrived first.
  const prev: TransferItem = state[ev.transferId] ?? {
    transferId: ev.transferId,
    direction: "recv",
    peerName: getLocale().transfer.unknownPeer,
    peerFingerprint: "",
    status: "active",
    currentFile: "",
    done: 0,
    size: 0,
    filesDone: 0,
    speed: 0,
    startedAt: action.at,
  };

  let next: TransferItem = prev;
  switch (ev.kind) {
    case "progress": {
      // Exponentially smooth same-file byte deltas over elapsed time.
      let speed = prev.speed;
      const sample = speedSamples.get(ev.transferId);
      if (sample && sample.file === ev.relPath && ev.done > sample.done) {
        const dt = (action.at - sample.at) / 1000;
        if (dt > 0) {
          const inst = (ev.done - sample.done) / dt;
          speed = speed === 0 ? inst : speed * 0.7 + inst * 0.3;
        }
      }
      speedSamples.set(ev.transferId, { done: ev.done, at: action.at, file: ev.relPath });
      next = {
        ...prev,
        status: prev.status === "paused" ? "paused" : "active",
        currentFile: ev.relPath,
        done: ev.done,
        size: ev.size,
        speed,
      };
      break;
    }
    case "fileCompleted":
      next = { ...prev, filesDone: prev.filesDone + 1, lastPath: ev.path };
      break;
    case "completed":
      speedSamples.delete(ev.transferId);
      next = { ...prev, status: "completed", speed: 0, done: prev.size };
      break;
    case "cancelled":
      speedSamples.delete(ev.transferId);
      next = { ...prev, status: "cancelled", speed: 0 };
      break;
    case "interrupted":
      speedSamples.delete(ev.transferId);
      next = {
        ...prev,
        status: "interrupted",
        speed: 0,
        // Resolve the error code locally and fall back to the engine's raw message.
        reason: formatErrorCode(ev.code, ev.detail, ev.reason),
      };
      break;
    case "ignored":
      speedSamples.delete(ev.transferId);
      next = {
        ...prev,
        status: "ignored",
        speed: 0,
        // Terminal state has no active file; clear any waiting-response placeholder.
        currentFile: "",
        reason: getLocale().transfer.ignoredReason,
      };
      break;
    case "rejected":
      speedSamples.delete(ev.transferId);
      next = {
        ...prev,
        status: "rejected",
        speed: 0,
        // Localize structured rejection codes from 1.4+ peers; preserve legacy fallback text.
        reason: formatErrorCode(
          ev.reasonCode,
          null,
          ev.reason ?? getLocale().transfer.rejectedDefault,
        ),
        pinRequired: ev.pinRequired,
      };
      break;
    // Peer pause and resume arrive as engine events; local actions use setPaused.
    case "paused":
      if (prev.status !== "active" && prev.status !== "paused") break;
      next = { ...prev, pausedByPeer: true, status: "paused", speed: 0 };
      break;
    case "resumed":
      if (prev.status !== "active" && prev.status !== "paused") break;
      next = { ...prev, pausedByPeer: false, status: runningStatus(prev.pausedLocal, false) };
      break;
  }
  return { ...state, [ev.transferId]: next };
  };
}

/** Unified application state and operations. */
export function useDeskmate() {
  const [self, setSelf] = useState<SelfInfoDto | null>(null);
  const [peers, setPeers] = useState<Record<string, PeerDto>>({});
  const [offers, setOffers] = useState<OfferDto[]>([]);
  const [texts, setTexts] = useState<TextMsg[]>([]);
  // Image-avatar Blob URLs keyed by hash and shared by peers and local device.
  const [avatarSrcs, setAvatarSrcs] = useState<Record<string, string>>({});
  // Bind speed samples and reducer to the same component instance.
  const speedSamples = useRef<SpeedSamples>(new Map());
  const reducer = useMemo(() => makeTransferReducer(speedSamples.current), []);
  const [transfers, dispatch] = useReducer(reducer, {});
  // Preserve offer peer names for creating accepted receive entries.
  const offersRef = useRef<OfferDto[]>([]);
  offersRef.current = offers;
  // A ref provides pre-event aggregate state when reporting terminal history.
  const transfersRef = useRef(transfers);
  transfersRef.current = transfers;
  // Track loading and loaded avatar hashes to prevent duplicate requests.
  const avatarSeen = useRef(new Set<string>());
  // Revoke all Blob URLs on unmount; stale avatar URLs remain small during a session.
  const avatarSrcsRef = useRef(avatarSrcs);
  avatarSrcsRef.current = avatarSrcs;
  useEffect(
    () => () => {
      for (const url of Object.values(avatarSrcsRef.current)) URL.revokeObjectURL(url);
    },
    [],
  );
  // Non-persistent peer PIN session cache keyed by fingerprint.
  const pinCache = useRef(new Map<string, string>());

  /** Returns a session-cached peer PIN through a stable callback. */
  const getPin = useCallback((fingerprint: string) => pinCache.current.get(fingerprint), []);
  /** Remembers a verified peer PIN for the current process. */
  const rememberPin = useCallback((fingerprint: string, pin: string) => {
    pinCache.current.set(fingerprint, pin);
  }, []);

  /** Loads avatar bytes into a Blob URL; self reads the local custom-avatar file. */
  const loadAvatar = (hash: string | null, isSelf = false) => {
    if (!hash || avatarSeen.current.has(hash)) return;
    avatarSeen.current.add(hash);
    api
      .getAvatarImage(isSelf ? undefined : hash)
      .then((bytes) => {
        if (bytes && bytes.length > 0) {
          setAvatarSrcs((prev) => ({ ...prev, [hash]: avatarBlobUrl(bytes) }));
        } else {
          // A cache miss remains retryable after avatar-ready arrives.
          avatarSeen.current.delete(hash);
        }
      })
      .catch(() => avatarSeen.current.delete(hash));
  };

  useEffect(() => {
    let alive = true;
    const unsubs: UnlistenFn[] = [];
    const add = (p: Promise<UnlistenFn>) =>
      p.then((u) => {
        // Immediately remove a late subscription from StrictMode's duplicate effect.
        if (alive) unsubs.push(u);
        else u();
      });

    // Browser-only previews may throw synchronously when listen has no Tauri runtime.
    if (!("__TAURI_INTERNALS__" in window)) {
      console.warn("Non-Tauri environment: skipping engine event subscriptions");
      return;
    }

    add(
      listen<PeerDto>(EVENTS.PEER_UP, (e) => {
        loadAvatar(avatarHashOf(e.payload.avatar));
        setPeers((prev) => ({ ...prev, [e.payload.fingerprint]: e.payload }));
      }),
    );
    // Reload the cache after a background avatar fetch completes.
    add(
      listen<{ fingerprint: string; hash: string }>(EVENTS.AVATAR_READY, (e) =>
        loadAvatar(e.payload.hash),
      ),
    );
    // Automatic trusted-device acceptance creates a receive entry without a dialog.
    add(
      listen<{ transferId: string; peerName: string }>(EVENTS.TRANSFER_AUTOSTART, (e) =>
        dispatch({
          type: "begin",
          transferId: e.payload.transferId,
          direction: "recv",
          peerName: e.payload.peerName,
          peerFingerprint: "",
        }),
      ),
    );
    add(
      listen<string>(EVENTS.PEER_DOWN, (e) =>
        setPeers((prev) => {
          const next = { ...prev };
          delete next[e.payload];
          return next;
        }),
      ),
    );
    add(listen<OfferDto>(EVENTS.TRANSFER_OFFER, (e) => setOffers((prev) => [...prev, e.payload])));
    add(
      listen<TransferEventDto>(EVENTS.TRANSFER_EVENT, (e) => {
        const ev = e.payload;
        if (ev.kind === "textReceived") {
          setTexts((prev) =>
            [
              {
                id: `${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
                direction: "in" as const,
                peerName: ev.fromName,
                text: ev.text,
                at: Date.now(),
              },
              ...prev,
            ].slice(0, 50),
          );
        } else {
          dispatch({ type: "event", event: ev, at: Date.now() });
          // Surface failures in the in-app notification tray. The transfer ID
          // keys each toast so duplicate terminal DTOs (engine event plus send
          // settlement) update one notification instead of stacking two.
          if (ev.kind === "ignored") {
            ANotification.warning({
              key: ev.transferId,
              message: getLocale().transfer.ignoredReason,
            });
          } else if (ev.kind === "interrupted") {
            ANotification.error({
              key: ev.transferId,
              message: getLocale().transfer.interruptedNotice,
              description: formatErrorCode(ev.code, ev.detail, ev.reason),
            });
          } else if (ev.kind === "rejected" && !ev.pinRequired) {
            // PIN-gated rejections open the retry flow instead of an error toast.
            ANotification.error({
              key: ev.transferId,
              message: getLocale().transfer.rejectedNotice,
              description: formatErrorCode(
                ev.reasonCode,
                null,
                ev.reason ?? getLocale().transfer.rejectedDefault,
              ),
            });
          }
          // Report terminal history from pre-event aggregate state and accumulated progress.
          if (
            ev.kind === "completed" ||
            ev.kind === "cancelled" ||
            ev.kind === "interrupted" ||
            ev.kind === "rejected"
          ) {
            const prev = transfersRef.current[ev.transferId];
            api
              .appendHistory({
                transferId: ev.transferId,
                direction: prev?.direction ?? "recv",
                peerName: prev?.peerName ?? getLocale().transfer.unknownPeer,
                status: ev.kind,
                filesDone: prev?.filesDone ?? 0,
                bytes: prev?.done ?? 0,
                at: Date.now(),
                lastPath: prev?.lastPath ?? null,
              })
              .catch(console.error);
          }
        }
      }),
    );

    // Initial snapshot.
    api
      .getSelfInfo()
      .then((info) => {
        if (!alive) return;
        setSelf(info);
        loadAvatar(avatarHashOf(info.avatar), true);
      })
      .catch(console.error);
    api
      .listPeers()
      .then((list) => {
        if (!alive) return;
        setPeers(Object.fromEntries(list.map((p) => [p.fingerprint, p])));
        list.forEach((p) => loadAvatar(avatarHashOf(p.avatar)));
      })
      .catch(console.error);

    return () => {
      alive = false;
      unsubs.forEach((u) => u());
    };
  }, []);

  /** Sends absolute file paths to a peer through a stable callback. */
  const sendFiles = useCallback(
    async (peer: PeerDto, paths: string[]) => {
      const transferId = await api.sendFiles(peer.fingerprint, paths, getPin(peer.fingerprint));
      dispatch({
        type: "begin",
        transferId,
        direction: "send",
        peerName: peer.name,
        peerFingerprint: peer.fingerprint,
      });
    },
    [getPin],
  );

  /** Stages a clipboard screenshot through raw IPC, then sends it as a file.
   * The stable callback is passed to the memoized TransferPanel. */
  const sendClipboardImage = useCallback(
    async (peer: PeerDto, fileName: string, bytes: Uint8Array) => {
      const staged = await api.stageClipboardImage(bytes);
      const transferId = await api.sendClipboardImage(
        peer.fingerprint,
        fileName,
        staged,
        getPin(peer.fingerprint),
      );
      dispatch({
        type: "begin",
        transferId,
        direction: "send",
        peerName: peer.name,
        peerFingerprint: peer.fingerprint,
      });
    },
    [getPin],
  );

  /** Responds to an offer with the selected overwrite behavior.
   *
   * Remove the dialog even when the response fails because timeout or disconnect;
   * otherwise it would permanently block later offers in the session. */
  const respondOffer = useCallback(
    async (offer: OfferDto, accept: boolean, opts?: { saveDir?: string; overwrite?: boolean }) => {
      try {
        await api.respondOffer(offer.offerId, accept, opts?.saveDir, opts?.overwrite ?? false);
        if (accept) {
          dispatch({
            type: "begin",
            transferId: offer.transferId,
            direction: "recv",
            peerName: offer.peerName,
            peerFingerprint: offer.peerFingerprint,
          });
        }
      } catch (e) {
        console.error("Failed to respond to incoming offer:", e);
      } finally {
        setOffers((prev) => prev.filter((o) => o.offerId !== offer.offerId));
      }
    },
    [],
  );

  /** Pauses a transfer and updates local state after engine confirmation. */
  const pauseTransfer = useCallback((transferId: string) => {
    api
      .pause(transferId)
      .then((ok) => {
        if (ok) dispatch({ type: "setPaused", transferId, paused: true });
      })
      .catch(console.error);
  }, []);

  /** Resumes a transfer symmetrically with pauseTransfer. */
  const resumeTransfer = useCallback((transferId: string) => {
    api
      .resume(transferId)
      .then((ok) => {
        if (ok) dispatch({ type: "setPaused", transferId, paused: false });
      })
      .catch(console.error);
  }, []);

  /** Records successfully sent text in the message stream. */
  const addSentText = useCallback((peerName: string, text: string) => {
    setTexts((prev) =>
      [
        {
          id: `${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
          direction: "out" as const,
          peerName,
          text,
          at: Date.now(),
        },
        ...prev,
      ].slice(0, 50),
    );
  }, []);

  /** Deletes one in-memory text message. */
  const removeText = useCallback((id: string) => {
    setTexts((prev) => prev.filter((m) => m.id !== id));
  }, []);

  /** Clears all in-memory text messages. */
  const clearTexts = useCallback(() => setTexts([]), []);

  /** Reloads local-device information and any updated avatar after settings save. */
  const refreshSelf = () => {
    api
      .getSelfInfo()
      .then((info) => {
        setSelf(info);
        loadAvatar(avatarHashOf(info.avatar), true);
      })
      .catch(console.error);
  };

  return {
    self,
    peers,
    offers,
    texts,
    transfers,
    avatarSrcs,
    sendFiles,
    sendClipboardImage,
    respondOffer,
    pauseTransfer,
    resumeTransfer,
    getPin,
    rememberPin,
    addSentText,
    removeText,
    clearTexts,
    refreshSelf,
    dispatch,
  };
}
