// Frontend types matching the camelCase DTOs from the src-tauri bridge.

/** Online peer. */
export interface PeerDto {
  deviceId: string;
  name: string;
  fingerprint: string;
  platform: string;
  addrs: string[];
  port: number;
  /** Built-in emoji avatar; null uses the initial style. */
  avatar: string | null;
  /** Operating-system version, or null for older peers. */
  osVersion: string | null;
}

/** Local-device information. */
export interface SelfInfoDto {
  name: string;
  deviceId: string;
  fingerprint: string;
  platform: string;
  port: number;
  downloadDir: string;
  avatar: string | null;
}

/** Transfer manifest file entry. */
export interface FileMetaDto {
  fileId: number;
  relPath: string;
  size: number;
}

/** Incoming transfer offer awaiting a decision. */
export interface OfferDto {
  offerId: string;
  transferId: string;
  peerName: string;
  peerFingerprint: string;
  peerPlatform: string;
  peerAvatar: string | null;
  files: FileMetaDto[];
  totalSize: number;
}

/** Transfer lifecycle event distinguished by kind. */
export type TransferEventDto =
  | { kind: "progress"; transferId: string; fileId: number; relPath: string; done: number; size: number }
  | { kind: "fileCompleted"; transferId: string; fileId: number; path: string; inlineImage: boolean }
  | { kind: "completed"; transferId: string }
  | { kind: "cancelled"; transferId: string }
  | { kind: "interrupted"; transferId: string; reason: string; code: string; detail: string | null }
  | { kind: "paused"; transferId: string }
  | { kind: "resumed"; transferId: string }
  | { kind: "ignored"; transferId: string }
  | { kind: "rejected"; transferId: string; reason: string | null; pinRequired: boolean; reasonCode: string | null }
  | { kind: "textReceived"; fromName: string; fromFingerprint: string; text: string };

/** File-name conflict policy: rename, overwrite, or ask. */
export type ConflictPolicy = "rename" | "overwrite" | "ask";

/** Trusted device accepted without confirmation. */
export interface TrustedDevice {
  fingerprint: string;
  /** Display name captured when the device was trusted. */
  name: string;
}

/** Application settings. */
export interface Settings {
  displayName: string | null;
  downloadDir: string;
  tcpPort: number;
  conflictPolicy: ConflictPolicy;
  /** Built-in emoji avatar; null uses the initial style. */
  avatar: string | null;
  /** Passive mode discovers peers without advertising this device. */
  passive: boolean;
  /** Launch at system startup. */
  autostart: boolean;
  /** Trusted-device allowlist for automatic acceptance. */
  trusted: TrustedDevice[];
  /** Optional pairing PIN required for incoming files and text. */
  pin: string | null;
  /** Automatically copy received text to the system clipboard. */
  autoCopyText: boolean;
  /** Global send-clipboard hotkey in Tauri syntax; null disables it. */
  sendClipboardHotkey: string | null;
  /** Global copy-and-send hotkey; null disables it. */
  copySendHotkey: string | null;
  /** Gitignore-style transfer rules, one per line. */
  ignoreRules: string;
  /** Interface language: "zh" or "en"; empty until first-run system detection. */
  language: string;
}

/** Built-in avatars selectable in settings and advertised during discovery. */
export const AVATARS = [
  "🦊", "🐱", "🐼", "🦉",
  "🐸", "🦄", "🐙", "🦈",
  "🐝", "🦜", "🐢", "🦔",
  "🐳", "🦁", "🐰", "🤖",
] as const;

/** Extracts a hash from `img:<hash>`; emoji and empty values return null. */
export function avatarHashOf(avatar: string | null | undefined): string | null {
  return avatar?.startsWith("img:") ? avatar.slice(4) : null;
}

/** Converts avatar bytes into a Blob URL suitable for an image source. */
export function avatarBlobUrl(bytes: number[]): string {
  return URL.createObjectURL(new Blob([new Uint8Array(bytes)], { type: "image/jpeg" }));
}

/** Receive preflight result. */
export interface PrecheckDto {
  /** Available target-disk bytes; null skips the capacity check. */
  freeBytes: number | null;
  /** Relative paths conflicting with existing target files. */
  conflicts: string[];
}

/** Frontend aggregate state for one transfer task. */
export interface TransferItem {
  transferId: string;
  direction: "send" | "recv";
  peerName: string;
  /** Peer fingerprint used to update the PIN session cache. */
  peerFingerprint: string;
  status: "active" | "paused" | "completed" | "cancelled" | "interrupted" | "rejected" | "ignored";
  /** File currently being transferred. */
  currentFile: string;
  done: number;
  size: number;
  /** Number of completed files. */
  filesDone: number;
  /** Smoothed speed in bytes per second. */
  speed: number;
  /** Absolute path of the last completed file for the reveal action. */
  lastPath?: string;
  /** Failure or rejection reason. */
  reason?: string;
  /** Rejected because the peer requires a pairing PIN. */
  pinRequired?: boolean;
  /** Local pause flag, independent from the peer pause flag. */
  pausedLocal?: boolean;
  /** Peer pause flag driven by engine Paused and Resumed events. */
  pausedByPeer?: boolean;
  startedAt: number;
}

/** Transfer history entry reported by the frontend and persisted by Rust. */
export interface HistoryEntry {
  transferId: string;
  direction: "send" | "recv";
  peerName: string;
  status: "completed" | "cancelled" | "interrupted" | "rejected";
  filesDone: number;
  bytes: number;
  /** Completion time in Unix milliseconds. */
  at: number;
  lastPath: string | null;
}

/** Text message in the shared incoming and outgoing stream. */
export interface TextMsg {
  id: string;
  /** Direction: in for received, out for sent. */
  direction: "in" | "out";
  /** Peer name, either the source or destination. */
  peerName: string;
  /** Text content; the file name for image messages. */
  text: string;
  at: number;
  /** Inline clipboard image rendered as a chat bubble instead of text. */
  image?: {
    /** Blob URL owned by the message stream and revoked on removal. */
    url: string;
    name: string;
  };
}

/** Formats byte counts, for example 1536 as "1.5 KB". */
export function humanBytes(n: number): string {
  const units = ["B", "KB", "MB", "GB", "TB"];
  let v = n;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i += 1;
  }
  return i === 0 ? `${n} B` : `${v.toFixed(1)} ${units[i]}`;
}
