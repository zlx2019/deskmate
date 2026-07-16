// 中文文案(权威键源): 英文在 en.ts, 两个文件按同一结构维护
// 带参数的文案用函数字段(不同语言语序不同, 整句在各自文件里组装)
// 小写英文短标题(settings / send text 等)是设计稿的装饰性风格, 两种语言一致

export const zh = {
  /** 顶栏 */
  header: {
    online: (n: number) => `${n} 台设备在线`,
    port: (p: number | string) => `端口 ${p}`,
    toLight: "切换到亮色",
    toSystem: "跟随系统主题",
    toDark: "切换到暗色",
    settings: "设置",
  },

  /** 搜寻地图 */
  radar: {
    myDevice: "我的设备",
    thisDevice: "THIS DEVICE",
    scanning: "正在搜寻附近的设备",
    dragHint: "拖拽文件到设备头像上即可发送",
    dropToSend: "松开即发送",
    dragToTarget: "拖到目标设备上发送",
  },

  /** 后端错误码 → 文案(键与 Rust 侧 code 一一对应, 细节参数由调用方拼接) */
  errors: {
    // 引擎传输错误(TransferError)
    io: "网络或磁盘读写出错",
    protocol: "协议错误",
    tls: "加密连接失败",
    peer_mismatch: "对方身份校验不通过",
    invalid_path: "文件路径不安全",
    source_not_found: "源文件不存在或不可读",
    rejected: "对方拒绝",
    hash_mismatch: "文件校验失败",
    cancelled: "已取消",
    timeout: "等待超时",
    bad_file_id: "协议错误(非法文件序号)",
    unknown_transfer: "任务不存在或已结束",
    duplicate_data_session: "任务已有进行中的连接",
    resume_unavailable: "无法续传",
    resume_offset_mismatch: "续传断点不一致",
    avatar_too_large: "图片超出大小上限",
    pin_required: "对方要求配对 PIN",
    // 对端结构化拒因(协议 1.4 reason_code)
    declined: "对方拒绝了本次传输",
    decision_timeout: "对方长时间未响应",
    duplicate_task: "对方已有同 ID 任务在进行",
    no_valid_files: "未选择任何有效文件",
    receiver_unavailable: "对方暂时无法接收",
    bad_transfer_id: "任务 ID 非法",
    // 命令层
    peer_offline: "对方设备已离线",
    no_files_selected: "未选择任何文件",
    screenshot_empty: "截图数据为空",
    screenshot_stage_missing: "截图暂存已失效, 请重新截图",
    retry_unavailable: "该任务无法重试(缺少原始参数)",
    offer_expired: "该请求已过期或已处理",
    session_gone: "会话已断开",
    download_dir_unavailable: "下载目录不可用",
    settings_save_failed: "保存设置失败",
    identity_update_failed: "更新身份失败",
    hotkey_invalid: "快捷键格式无效",
    hotkey_conflict: "快捷键注册失败(可能与其他应用冲突)",
    avatar_empty: "图片数据为空",
    internal: "内部错误",
  },

  /** 传输面板(状态 / 卡片 / 文字消息区) */
  transfer: {
    status: {
      active: "传输中",
      paused: "已暂停",
      completed: "已完成",
      cancelled: "已取消",
      interrupted: "已中断",
      rejected: "被拒绝",
    },
    tabTasks: "传输任务",
    tabHistory: "互传记录",
    sendTo: "发往 ",
    recvFrom: "来自 ",
    pause: "暂停",
    pausedByPeer: "对方已暂停",
    resume: "继续",
    cancel: "取消",
    reveal: "显示",
    resumeSend: "续传",
    enterPin: "输入 PIN",
    files: (n: number) => `${n} 个文件`,
    eta: (t: string) => `剩余 ${t}`,
    emptyTasks: "拖拽文件到地图上的设备即可发送",
    textSection: "文字消息",
    emptyTexts: "暂无文本消息",
    clearTexts: "清空文字消息",
    copy: "复制",
    rejectedDefault: "对方拒绝",
    waitingResponse: "等待对方响应…",
    unknownPeer: "对方设备",
  },

  /** 互传记录 */
  history: {
    loading: "加载中…",
    empty: "暂无互传记录",
    total: (n: number) => `共 ${n} 条`,
    clear: "清空互传记录",
  },

  /** 消息输入行(含全局快捷键发剪贴板的系统通知) */
  composer: {
    to: "发给",
    placeholder: "输入消息, Enter 发送(逐字原样送达)",
    pinPlaceholder: "输入对方的配对 PIN 后重新发送",
    pinRequired: "对方要求配对 PIN",
    send: "发送",
    noPeers: "暂无在线设备, 无法发送消息",
    screenshotSendingTip: "截图发送中, 见传输任务",
    notifyScreenshotSending: "截图发送中",
    notifyScreenshotFailed: "截图发送失败",
    notifyTo: (name: string) => `发往 ${name}`,
    notifyNoPeer: "没有在线设备, 剪贴板未发送",
    notifyNoClip: "剪贴板没有文本或截图, 未发送",
    notifyClipSent: "剪贴板已送达",
    notifyClipFailed: "剪贴板发送失败",
    notifyOpenApp: "打开 deskmate 查看详情",
  },

  /** 接收确认弹窗 */
  offer: {
    title: "incoming transfer",
    wantsToSend: "想向你发送文件",
    filesSummary: (n: number, size: string) => `${n} files · ${size}`,
    saveTo: "save to",
    change: "更改…",
    pickDirTitle: "选择保存位置",
    notEnough: (free: string, need: string) =>
      `磁盘空间不足: 可用 ${free}, 需要 ${need}, 请更换保存位置`,
    freeSpace: (free: string) => `可用空间 ${free}`,
    conflictAsk: (n: number) => `${n} 个文件与目标目录同名, 如何处理?`,
    conflictRename: "自动重命名",
    conflictOverwrite: "覆盖旧文件",
    conflictNotice: (n: number, overwrite: boolean) =>
      `${n} 个同名文件将${overwrite ? "被覆盖" : "自动重命名"}`,
    reject: "拒绝",
    accept: "接收",
  },

  /** 节点操作弹窗 */
  peer: {
    title: "send to peer",
    sendFiles: "发送文件…",
    sendFolder: "发送文件夹…",
    pickFilesTitle: "选择要发送的文件",
    pickFolderTitle: "选择要发送的文件夹",
    trustLabel: "信任此设备",
    trustHint: "它发来的文件将免确认自动接收到默认下载目录",
    sendTextSection: "send text",
    textPlaceholder: "输入要发送的文本, 将逐字原样送达(保留空格与换行)",
    pinPlaceholder: "输入对方的配对 PIN 后重新发送",
    pinRequired: "对方要求配对 PIN",
    sendClipboard: "发送剪贴板",
    sendText: "发送文本",
    sending: "发送中…",
    delivered: "已送达",
    clipDelivered: "剪贴板已送达",
    noClip: "剪贴板没有文本或截图",
  },

  /** PIN 重试弹窗 */
  pinModal: {
    title: "pairing pin",
    // 完整句为「<对方昵称> 启用了配对 PIN, 请输入后重试」, 昵称在界面上加粗
    promptSuffix: " 启用了配对 PIN, 请输入后重试",
    placeholder: "对方设置页显示的 PIN",
    cancel: "取消",
    retry: "重试发送",
  },

  /** 设置弹窗 */
  settings: {
    title: "settings",
    tabs: {
      general: "通用",
      user: "用户",
      security: "安全",
      hotkey: "快捷键",
    },
    downloadDir: "下载目录",
    choose: "选择…",
    pickDirTitle: "选择默认下载目录",
    conflict: "同名文件处理",
    conflictRename: "重命名",
    conflictOverwrite: "覆盖",
    conflictAsk: "询问",
    port: "监听端口",
    language: "语言",
    autoCopy: "自动复制",
    autostart: "开机自启",
    fingerprint: "device fingerprint",
    copyHint: "点击复制",
    nickname: "昵称",
    nicknamePlaceholder: "默认为主机名",
    avatar: "头像",
    initialStyle: "首字母样式",
    uploadAvatar: "上传图片头像",
    canvasError: "Canvas 不可用",
    encodeError: "图片编码失败",
    pin: "配对 PIN",
    pinPlaceholder: "不设置表示无需配对",
    trustedDevices: "受信设备(免确认自动接收)",
    remove: "移除",
    stealth: "隐身模式",
    hotkeyLabel: "发送剪贴板",
    hotkeyHint:
      "全局生效: 在任何应用按下, 即把剪贴板文本发给消息框选中的设备。点击输入框后按组合键设置, Backspace 清除。",
    hotkeyRecording: "请按下组合键…",
    hotkeyUnset: "未设置",
    loading: "加载中…",
    cancel: "取消",
    save: "保存",
  },

  /** 清理小件 */
  clear: {
    confirm: "确认清空",
    delete: "删除",
  },
};
