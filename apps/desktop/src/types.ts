// 与 src-tauri 桥接层 DTO 对齐的前端类型定义(camelCase 序列化)

/** 在线节点 */
export interface PeerDto {
  deviceId: string;
  name: string;
  fingerprint: string;
  platform: string;
  addrs: string[];
  port: number;
  /** 内置头像(emoji); null 时用首字母样式 */
  avatar: string | null;
  /** 操作系统版本描述(如 "Mac OS 15.3.1"; 旧版本对端为 null) */
  osVersion: string | null;
}

/** 本机信息 */
export interface SelfInfoDto {
  name: string;
  deviceId: string;
  fingerprint: string;
  platform: string;
  port: number;
  downloadDir: string;
  avatar: string | null;
}

/** 传输清单文件项 */
export interface FileMetaDto {
  fileId: number;
  relPath: string;
  size: number;
}

/** 待决策的接收请求 */
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

/** 传输过程事件(kind 区分) */
export type TransferEventDto =
  | { kind: "progress"; transferId: string; fileId: number; relPath: string; done: number; size: number }
  | { kind: "fileCompleted"; transferId: string; fileId: number; path: string }
  | { kind: "completed"; transferId: string }
  | { kind: "cancelled"; transferId: string }
  | { kind: "interrupted"; transferId: string; reason: string; code: string; detail: string | null }
  | { kind: "paused"; transferId: string }
  | { kind: "resumed"; transferId: string }
  | { kind: "rejected"; transferId: string; reason: string | null; pinRequired: boolean; reasonCode: string | null }
  | { kind: "textReceived"; fromName: string; fromFingerprint: string; text: string };

/** 同名冲突策略: 自动重命名 / 覆盖 / 每次询问 */
export type ConflictPolicy = "rename" | "overwrite" | "ask";

/** 受信设备(免确认自动接收) */
export interface TrustedDevice {
  fingerprint: string;
  /** 加入时的显示名(仅展示) */
  name: string;
}

/** 应用设置 */
export interface Settings {
  displayName: string | null;
  downloadDir: string;
  tcpPort: number;
  conflictPolicy: ConflictPolicy;
  /** 内置头像(emoji); null 表示用首字母样式 */
  avatar: string | null;
  /** 隐身模式: 只看别人不被看见(保存即时生效) */
  passive: boolean;
  /** 开机自启(保存后即时生效) */
  autostart: boolean;
  /** 受信设备白名单(免确认自动接收) */
  trusted: TrustedDevice[];
  /** 配对 PIN: 启用后对方发文件/文本必须携带正确 PIN(null 关闭, 即时生效) */
  pin: string | null;
  /** 收到文本时自动复制到系统剪贴板(即时生效) */
  autoCopyText: boolean;
  /** 发送剪贴板的全局快捷键(null 关闭; Tauri 语法, 如 "CmdOrCtrl+Shift+D") */
  sendClipboardHotkey: string | null;
  /** 界面语言: "zh" / "en"; 空表示未初始化(首启按系统语言检测后写入) */
  language: string;
}

/** 内置头像库(设置页挑选, 随发现报文广播) */
export const AVATARS = [
  "🦊", "🐱", "🐼", "🦉",
  "🐸", "🦄", "🐙", "🦈",
  "🐝", "🦜", "🐢", "🦔",
  "🐳", "🦁", "🐰", "🤖",
] as const;

/** 从头像字段提取图片哈希: "img:<hash>" → hash, 其余(emoji/空)→ null */
export function avatarHashOf(avatar: string | null | undefined): string | null {
  return avatar?.startsWith("img:") ? avatar.slice(4) : null;
}

/** 头像字节 → 可作 img src 的 Blob URL */
export function avatarBlobUrl(bytes: number[]): string {
  return URL.createObjectURL(new Blob([new Uint8Array(bytes)], { type: "image/jpeg" }));
}

/** 接收预检结果 */
export interface PrecheckDto {
  /** 目标磁盘可用字节数; null 表示查询失败(跳过空间校验) */
  freeBytes: number | null;
  /** 与目标目录已有文件同名的相对路径列表 */
  conflicts: string[];
}

/** 传输任务的前端聚合状态 */
export interface TransferItem {
  transferId: string;
  direction: "send" | "recv";
  peerName: string;
  /** 对端指纹(PIN 重试成功后回写会话缓存用; 兜底条目为空串) */
  peerFingerprint: string;
  status: "active" | "paused" | "completed" | "cancelled" | "interrupted" | "rejected";
  /** 当前正在传的文件 */
  currentFile: string;
  done: number;
  size: number;
  /** 已完成文件数 */
  filesDone: number;
  /** 平滑速度 bytes/s */
  speed: number;
  /** 最后完成文件的绝对路径(“显示”按钮用) */
  lastPath?: string;
  /** 失败/拒绝原因 */
  reason?: string;
  /** 被拒且对方要求配对 PIN(显示"输入 PIN"重试按钮) */
  pinRequired?: boolean;
  /** 本端按下了暂停(与对端标记独立, 双方都恢复才回到 active) */
  pausedLocal?: boolean;
  /** 对端按下了暂停(引擎 Paused/Resumed 事件驱动; 此间本端"继续"无效) */
  pausedByPeer?: boolean;
  startedAt: number;
}

/** 一条传输历史(终态时由前端上报, Rust 持久化) */
export interface HistoryEntry {
  transferId: string;
  direction: "send" | "recv";
  peerName: string;
  status: "completed" | "cancelled" | "interrupted" | "rejected";
  filesDone: number;
  bytes: number;
  /** 结束时间(unix 毫秒) */
  at: number;
  lastPath: string | null;
}

/** 文字消息(收到的与发出的都进同一消息流) */
export interface TextMsg {
  id: string;
  /** 方向: in=收到, out=发出 */
  direction: "in" | "out";
  /** 对端设备名(in 为来源, out 为目标) */
  peerName: string;
  text: string;
  at: number;
}

/** 字节数格式化: 1536 → "1.5 KB" */
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
