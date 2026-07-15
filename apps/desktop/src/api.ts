// Tauri commands 的类型化封装

import { invoke } from "@tauri-apps/api/core";
import type { HistoryEntry, PeerDto, PrecheckDto, SelfInfoDto, Settings } from "./types";

export const api = {
  /** 本机信息 */
  getSelfInfo: () => invoke<SelfInfoDto>("get_self_info"),
  /** 在线节点快照 */
  listPeers: () => invoke<PeerDto[]>("list_peers"),
  /** 发送文件/目录, 返回任务 ID(进度走 transfer-event 事件) */
  sendFiles: (fingerprint: string, paths: string[], pin?: string) =>
    invoke<string>("send_files_to", { fingerprint, paths, pin: pin ?? null }),
  /** 系统窗口材质(vibrancy/mica)是否生效, 决定是否启用半透明背景 */
  windowEffectsActive: () => invoke<boolean>("window_effects_active"),
  /** 发送文本(逐字节一致); 返回值 pinRequired 表示对端要求配对 PIN */
  sendText: (fingerprint: string, text: string, pin?: string) =>
    invoke<{ pinRequired: boolean }>("send_text_to", {
      fingerprint,
      text,
      pin: pin ?? null,
    }),
  /** 用给定 PIN 重试被拒的发送任务(复用原进度条目) */
  retrySend: (transferId: string, pin?: string) =>
    invoke<void>("retry_send_transfer", { transferId, pin: pin ?? null }),
  /** 应答接收请求; overwrite 为本次的同名冲突决策(true 覆盖 / false 自动重命名) */
  respondOffer: (offerId: string, accept: boolean, saveDir?: string, overwrite = false) =>
    invoke<void>("respond_offer", { offerId, accept, saveDir: saveDir ?? null, overwrite }),
  /** 接收前预检: 目标目录磁盘可用空间 + 同名冲突清单(dir 为空用默认下载目录) */
  precheckReceive: (dir: string | undefined, relPaths: string[]) =>
    invoke<PrecheckDto>("precheck_receive", { dir: dir ?? null, relPaths }),
  /** 暂停/恢复/取消传输 */
  pause: (transferId: string) => invoke<boolean>("pause_transfer", { transferId }),
  resume: (transferId: string) => invoke<boolean>("resume_transfer", { transferId }),
  cancel: (transferId: string) => invoke<boolean>("cancel_transfer", { transferId }),
  /** 续传意外中断的发送任务(补发缺失段) */
  resumeSend: (transferId: string) => invoke<void>("resume_send_transfer", { transferId }),
  /** 设置读写 */
  getSettings: () => invoke<Settings>("get_settings"),
  saveSettings: (settings: Settings) => invoke<void>("save_settings", { settings }),
  /** 传输历史: 读取(最新在前)、终态上报、删除单条、清空 */
  getHistory: () => invoke<HistoryEntry[]>("get_history"),
  appendHistory: (entry: HistoryEntry) => invoke<void>("append_history", { entry }),
  deleteHistory: (transferId: string) => invoke<void>("delete_history", { transferId }),
  clearHistory: () => invoke<void>("clear_history"),
  /** 上传本机自定义头像(前端压缩后的 JPEG 字节, 重启后生效) */
  setAvatarImage: (data: number[]) => invoke<void>("set_avatar_image", { data }),
  /** 读取头像字节: hash 缺省取本机自定义头像, 传入则查对端缓存(未命中 null) */
  getAvatarImage: (hash?: string) =>
    invoke<number[] | null>("get_avatar_image", { hash: hash ?? null }),
  /** 系统通知(未聚焦才发; 窗口可能隐藏的场景反馈用, 如快捷键发送结果) */
  notify: (title: string, body: string) => invoke<void>("notify", { title, body }),
  /** 暂存剪贴板截图字节(raw 二进制通道, 避免大图 JSON 序列化膨胀), 返回暂存 ID */
  stageClipboardImage: (data: Uint8Array) => invoke<string>("stage_clipboard_image", data),
  /** 发送已暂存的剪贴板截图(走文件传输链), 返回任务 ID */
  sendClipboardImage: (fingerprint: string, fileName: string, staged: string, pin?: string) =>
    invoke<string>("send_clipboard_image", { fingerprint, fileName, staged, pin: pin ?? null }),
};
