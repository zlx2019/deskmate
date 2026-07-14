# deskmate desktop

Tauri 2 + React 19 + Tailwind CSS 4 桌面端,包管理器使用 **pnpm**。

## 常用命令

```bash
pnpm install         # 安装依赖(postinstall 白名单见 package.json 的 pnpm.onlyBuiltDependencies)
pnpm tauri dev       # 开发模式: 启动 Vite dev server + Tauri 窗口(热更新)
pnpm tauri build     # 产出安装包(M4 里程碑正式使用)
pnpm build           # 仅构建前端静态资源(tsc 类型检查 + vite build)
```

> macOS 15+ 首次运行会弹"本地网络"授权,必须允许,否则 mDNS 设备发现静默失败。

## 目录结构

```text
├── Tauri.toml           # Tauri 应用配置(TOML 格式, 依赖 config-toml feature)
├── src-tauri/           # Rust 应用壳
│   ├── src/lib.rs       #   入口: 插件注册、引擎启动、commands 挂载
│   ├── src/bridge.rs    #   引擎桥接: deskmate-core 事件 → Tauri 事件(DTO 定义)
│   ├── src/commands.rs  #   前端可调用的全部 commands
│   ├── src/state.rs     #   运行时共享状态(AppState)
│   ├── src/settings.rs  #   设置持久化(settings.json)
│   ├── Info.plist       #   macOS 本地网络权限 + Bonjour 服务声明
│   └── capabilities/    #   Tauri 权限清单
└── src/                 # React 前端
    ├── hooks/useDeskmate.ts     # 状态核心: 事件订阅 + 传输聚合 reducer
    ├── components/Radar.tsx     # 雷达界面(扫描扇/节点环形布局/拖拽命中)
    ├── components/TransferPanel.tsx  # 传输列表 + 文本收件箱
    ├── components/Modals.tsx    # 接收确认 / 节点操作 / 设置 弹窗
    ├── api.ts                   # commands 类型化封装
    └── types.ts                 # 与桥接层对齐的 DTO 类型
```

前端事件约定(Rust → JS): `peer-up` / `peer-down` / `transfer-offer` / `transfer-event`,
定义见 `src-tauri/src/bridge.rs` 头部注释。
