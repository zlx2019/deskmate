/** Tauri 事件名: 与 src-tauri/src/bridge.rs 的 events 模块一一对应, 改名必须两端同步 */
export const EVENTS = {
  /** 节点上线(载荷 PeerDto) */
  PEER_UP: "peer-up",
  /** 节点下线(载荷为指纹字符串) */
  PEER_DOWN: "peer-down",
  /** 收到传输请求, 等待用户决策(载荷 OfferDto) */
  TRANSFER_OFFER: "transfer-offer",
  /** 传输过程事件(载荷 TransferEventDto) */
  TRANSFER_EVENT: "transfer-event",
  /** 白名单自动接收已开始(前端据此建进度条目) */
  TRANSFER_AUTOSTART: "transfer-autostart",
  /** 对端头像缓存就绪(前端重新读取缓存) */
  AVATAR_READY: "avatar-ready",
  /** 全局快捷键触发: 读剪贴板发给消息框选中设备(无载荷) */
  HOTKEY_SEND_CLIPBOARD: "hotkey-send-clipboard",
} as const;
