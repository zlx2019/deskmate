// 中文文案(权威键源): 英文在 en.ts, 两个文件按同一结构维护
// 带参数的文案用函数字段(不同语言语序不同, 整句在各自文件里组装)
// 小写英文短标题(settings / send text 等)是设计稿的装饰性风格, 两种语言一致

export const zh = {
  /** 顶栏 */
  header: {
    online: (n: number) => `${n} 台设备在线`,
    port: (p: number | string) => `端口 ${p}`,
    toLight: "切换到亮色",
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
