<p align="center">
  <img src="./assets/logo.svg" width="96" alt="deskmate logo" />
</p>

<h1 align="center">Deskmate</h1>

<p align="center">
  在局域网内传输文件和文本，就像给同桌递小纸条一样简单。
</p>

<p align="center">
  <a href="https://github.com/zlx2019/deskmate/actions/workflows/ci.yml"><img src="https://github.com/zlx2019/deskmate/actions/workflows/ci.yml/badge.svg" alt="CI" /></a>
  <a href="https://github.com/zlx2019/deskmate/releases"><img src="https://img.shields.io/github/v/release/zlx2019/deskmate?include_prereleases" alt="Release" /></a>
  <a href="./LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="License: MIT" /></a>
  <img src="https://img.shields.io/badge/platform-macOS%20%7C%20Windows%20%7C%20Linux-8b96ff" alt="Platform" />
</p>

<p align="center">
  <a href="./README.md">English</a> · <b>简体中文</b>
</p>

---

Deskmate 运行的每台设备, 都是局域网络中的一个节点，节点与节点之间互相发现。节点上线后以气泡形式实时出现在其他节点主界面上，只需要简单拖拽即可向目标节点发送任何数据，数据经端到端 TLS 1.3 加密通道全速直达。


<p align="center">
  <img src="./assets/screenshot-day.png" width="49.5%" alt="deskmate 白天 —— 青青草原地图" />
  <img src="./assets/screenshot-night.png" width="49.5%" alt="deskmate 夜晚 —— 星空夜色地图" />
</p>

## ✨ 特性

- 🌐 **局域网P2P** —— 局域网设备互相发现, 上线/下线实时更新。
- 🖱️ **傻瓜式操作** —— 将文件/文件夹拖拽至目标设备头像上，对方确认后即可开始传输。
- ⏯️ **传输控制** —— 双端均可暂停 / 继续 / 取消,实时速度与剩余时间。
- 🔁 **断点续传** —— 意外断连保留已收数据,一键从断点字节续传,整文件 BLAKE3 校验兜底。
- 📋 **剪贴板同步** —— 可自动复制到剪切板, 实现一键共享。
- 🔐 **数据安全** —— TLS 1.3 双向认证、证书指纹即设备身份;可选配对 PIN(含暴力破解限速)与免确认信任白名单。
- 🌗 **主题配色** —— 支持亮色主题青绿草原风格，也支持暗色主题的黑色夜空主题。
- 🎨 **个性化** —— emoji 或自定义图片头像、自定义昵称、面板背景色随心换。
- 🖥️ **桌面原生体验** —— 系统托盘常驻、系统通知、开机自启、单实例锁、传输历史。
- 🪶 **轻量级** —— Tauri 2 + Rust, 安装包不足 10 MB。

## 📥 安装

从 [Releases](https://github.com/zlx2019/deskmate/releases) 下载对应平台的安装包:

| 平台 | 产物 |
|---|---|
| macOS(Apple Silicon / Intel) | `Deskmate_x.y.z_aarch64.dmg` / `Deskmate_x.y.z_x64.dmg` |
| Windows | `Deskmate_x.y.z_x64-setup.exe`(NSIS,自动注册防火墙规则) |
| Linux | `Deskmate_x.y.z_amd64.AppImage` / `.deb` |

> 当前构建未做代码签名。macOS 首次打开请右键 →「打开」(或执行 `xattr -cr /Applications/Deskmate.app`);Windows SmartScreen 可能同样需要手动确认。

## 🔨 开发

前置要求:**Rust ≥ 1.96**、**Node ≥ 22**、**pnpm**。Linux 还需要 Tauri 的系统依赖(`libwebkit2gtk-4.1-dev`、`libayatana-appindicator3-dev`、`librsvg2-dev` 等)。

```bash
git clone https://github.com/zlx2019/deskmate.git
cd deskmate/apps/desktop
pnpm install
pnpm tauri dev     # 开发模式(热更新)
pnpm tauri build   # 产出当前平台安装包
```

引擎是一个不依赖 UI 的 Rust 库(`deps/deskmate-core`),桌面端与 CLI(`deps/deskmate-cli`)共用,后者适合协议联调:

```bash
cargo run -p deskmate-cli -- listen    # 作为接收端
cargo run -p deskmate-cli -- scan     # 列出附近设备
cargo nextest run --workspace         # 跑测试套件
```


## ❓ 常见问题

**macOS 提示应用已损坏 / 来自身份不明的开发者。**
构建暂未公证。首次右键 →「打开」即可,或执行 `xattr -cr /Applications/Deskmate.app` 清除隔离标记。

**macOS 上一直发现不了设备。**
macOS 15+ 首次启动会请求「本地网络」权限 —— 必须允许,否则设备发现会静默失败。可在 系统设置 → 隐私与安全性 → 本地网络 中重新开启。

**Windows 上一直发现不了设备。**
设备发现需要防火墙入站规则。NSIS 安装器会自动注册;若直接运行绿色版二进制,请在 Windows 弹窗时为 `Deskmate.exe` 放行专用网络。

**为什么自研 TCP 协议而不用 HTTP?**
局域网 RTT 亚毫秒、丢包近乎为零,HTTP 的礼节性开销换不来收益;而暂停/继续/取消这类精确控制语义,在带帧的 TCP + TLS 通道上实现得干净得多。详见 [docs/PLAN.md](./docs/PLAN.md)。

**Linux 需要哪些运行时库?**
`webkit2gtk-4.1` 与 `libayatana-appindicator3` —— `.deb` 已声明依赖,AppImage 则内置。

## 📄 许可证

[MIT](./LICENSE)

界面基于 [animal-island-ui](https://github.com/guokaigdg/animal-island-ui) 构建,该组件库采用 [CC BY-NC 4.0](https://creativecommons.org/licenses/by-nc/4.0/) 许可(禁止商业用途)。如需商用 Deskmate,请先替换该组件库。
