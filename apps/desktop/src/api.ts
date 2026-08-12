// Typed wrappers around Tauri commands.

import { invoke } from "@tauri-apps/api/core";
import type { HistoryEntry, PeerDto, PrecheckDto, SelfInfoDto, Settings } from "./types";

export const api = {
  /** Local-device information. */
  getSelfInfo: () => invoke<SelfInfoDto>("get_self_info"),
  /** Online peer snapshot. */
  listPeers: () => invoke<PeerDto[]>("list_peers"),
  /** Sends files or directories and returns a task ID. */
  sendFiles: (fingerprint: string, paths: string[], pin?: string) =>
    invoke<string>("send_files_to", { fingerprint, paths, pin: pin ?? null }),
  /** Returns copied clipboard file paths for hotkey sends. */
  readClipboardFiles: () => invoke<string[]>("read_clipboard_files"),
  /** Sends byte-identical text and reports whether the peer requires a PIN. */
  sendText: (fingerprint: string, text: string, pin?: string) =>
    invoke<{ pinRequired: boolean }>("send_text_to", {
      fingerprint,
      text,
      pin: pin ?? null,
    }),
  /** Retries a rejected send with a PIN and reuses its progress entry. */
  retrySend: (transferId: string, pin?: string) =>
    invoke<void>("retry_send_transfer", { transferId, pin: pin ?? null }),
  /** Responds to an offer with an overwrite or automatic-rename decision. */
  respondOffer: (offerId: string, accept: boolean, saveDir?: string, overwrite = false) =>
    invoke<void>("respond_offer", { offerId, accept, saveDir: saveDir ?? null, overwrite }),
  /** Checks target-disk capacity and file-name conflicts before receiving. */
  precheckReceive: (dir: string | undefined, relPaths: string[]) =>
    invoke<PrecheckDto>("precheck_receive", { dir: dir ?? null, relPaths }),
  /** Pauses, resumes, or cancels a transfer. */
  pause: (transferId: string) => invoke<boolean>("pause_transfer", { transferId }),
  resume: (transferId: string) => invoke<boolean>("resume_transfer", { transferId }),
  cancel: (transferId: string) => invoke<boolean>("cancel_transfer", { transferId }),
  /** Resumes an interrupted send by retransmitting missing ranges. */
  resumeSend: (transferId: string) => invoke<void>("resume_send_transfer", { transferId }),
  /** Reads and writes settings. */
  getSettings: () => invoke<Settings>("get_settings"),
  saveSettings: (settings: Settings) => invoke<void>("save_settings", { settings }),
  /** Reads, appends, deletes, and clears transfer history. */
  getHistory: () => invoke<HistoryEntry[]>("get_history"),
  appendHistory: (entry: HistoryEntry) => invoke<void>("append_history", { entry }),
  deleteHistory: (transferId: string) => invoke<void>("delete_history", { transferId }),
  clearHistory: () => invoke<void>("clear_history"),
  /** Uploads frontend-compressed custom-avatar JPEG bytes. */
  setAvatarImage: (data: number[]) => invoke<void>("set_avatar_image", { data }),
  /** Reads the local custom avatar or a cached peer avatar by hash. */
  getAvatarImage: (hash?: string) =>
    invoke<number[] | null>("get_avatar_image", { hash: hash ?? null }),
  /** Sends a system notification when the window is unfocused. */
  notify: (title: string, body: string) => invoke<void>("notify", { title, body }),
  /** Reads a received inline clipboard image as raw bytes for chat display. */
  readInlineImage: (path: string) => invoke<ArrayBuffer>("read_inline_image", { path }),
  /** Stages screenshot bytes through raw IPC and returns a staging ID. */
  stageClipboardImage: (data: Uint8Array) => invoke<string>("stage_clipboard_image", data),
  /** Sends a staged screenshot through the file-transfer flow. */
  sendClipboardImage: (fingerprint: string, fileName: string, staged: string, pin?: string) =>
    invoke<string>("send_clipboard_image", { fingerprint, fileName, staged, pin: pin ?? null }),
};
