// deskmate 主界面: 顶栏 + 雷达 + 传输面板, 以及弹窗与全局文件拖拽

import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useDeskmate } from "./hooks/useDeskmate";
import { Radar } from "./components/Radar";
import { TransferPanel } from "./components/TransferPanel";
import {
  OfferModal,
  PeerActionModal,
  PinModal,
  SettingsModal,
} from "./components/modals";
import { api } from "./api";
import { avatarHashOf, type PeerDto, type SelfInfoDto, type TransferItem } from "./types";
import logoUrl from "./assets/deskmate-logo.svg";

/** 顶栏(设计稿风: 方块 logo + 在线数 + 端口 badge + 主题切换 + 设置)
 * memo: 传输高频更新时 props 未变即跳过 */
const Header = memo(function Header({
  self,
  peerCount,
  theme,
  onToggleTheme,
  onOpenSettings,
}: {
  self: SelfInfoDto | null;
  peerCount: number;
  theme: "dark" | "light";
  onToggleTheme: () => void;
  onOpenSettings: () => void;
}) {
  return (
    <header className="flex h-13 shrink-0 items-center gap-3 border-b border-line bg-panel px-4 transition-colors duration-300">
      <div className="flex items-center gap-2 text-[15px] font-medium tracking-[0.06em] text-fog">
        <img src={logoUrl} alt="" className="size-6" draggable={false} />
        Deskmate
      </div>
      <div className="ml-auto flex items-center gap-3">
        <span className="flex items-center gap-1.5 text-[13px] text-mist">
          <span className="size-2 rounded-full bg-live" />
          <span className="font-medium text-fog">{peerCount}</span> 台设备在线
        </span>
        <span className="rounded-md border border-line px-2 py-1 text-xs text-faint">
          端口 {self?.port ?? "…"}
        </span>
        <button
          onClick={onToggleTheme}
          title={theme === "dark" ? "切换到亮色" : "切换到暗色"}
          className="flex size-8 cursor-pointer items-center justify-center rounded-lg border border-line text-fog transition-colors hover:border-line-2"
        >
          {theme === "dark" ? (
            /* 太阳: 当前暗色, 点击去亮色 */
            <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round">
              <circle cx="12" cy="12" r="4" />
              <path d="M12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4" />
            </svg>
          ) : (
            /* 月亮: 当前亮色, 点击回暗色 */
            <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round">
              <path d="M21 12.8A9 9 0 1 1 11.2 3a7 7 0 0 0 9.8 9.8z" />
            </svg>
          )}
        </button>
        <button
          onClick={onOpenSettings}
          title="设置"
          className="flex size-8 cursor-pointer items-center justify-center rounded-lg text-mist transition-colors hover:text-sonar"
        >
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8">
            <circle cx="12" cy="12" r="3" />
            <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09a1.65 1.65 0 0 0-1-1.51 1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09a1.65 1.65 0 0 0 1.51-1 1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33h.01a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51h.01a1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82v.01a1.65 1.65 0 0 0 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" />
          </svg>
        </button>
      </div>
    </header>
  );
});

/** 物理坐标(拖拽事件)→ CSS 坐标 → 命中的节点指纹 */
function hitPeer(pos: { x: number; y: number }): string | null {
  const scale = window.devicePixelRatio || 1;
  const el = document.elementFromPoint(pos.x / scale, pos.y / scale);
  return el?.closest?.("[data-peer]")?.getAttribute("data-peer") ?? null;
}

/** 应用根组件 */
export default function App() {
  const dm = useDeskmate();
  const [activePeer, setActivePeer] = useState<PeerDto | null>(null);
  const [showSettings, setShowSettings] = useState(false);
  // 待输入 PIN 重试的被拒任务
  const [pinRetry, setPinRetry] = useState<TransferItem | null>(null);
  // 主题偏好: localStorage 持久化, data-theme 驱动 CSS 变量整体换肤
  const [theme, setTheme] = useState<"dark" | "light">(() =>
    localStorage.getItem("dm-theme") === "light" ? "light" : "dark",
  );

  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    localStorage.setItem("dm-theme", theme);
    // 原生窗口 chrome(标题栏/边框)与内容主题同步(Windows 上差异明显)
    if ("__TAURI_INTERNALS__" in window) {
      getCurrentWindow().setTheme(theme).catch(console.error);
    }
  }, [theme]);
  const [dragging, setDragging] = useState(false);
  const [dragHover, setDragHover] = useState<string | null>(null);

  // 拖拽回调里需要最新的 peers 与 sendFiles, 用 ref 透传避免重复注册
  const peersRef = useRef(dm.peers);
  peersRef.current = dm.peers;
  const sendFilesRef = useRef(dm.sendFiles);
  sendFilesRef.current = dm.sendFiles;

  // 全局文件拖拽: over 时高亮命中的节点, drop 命中即发送
  useEffect(() => {
    let alive = true;
    let unlisten: (() => void) | undefined;
    // 纯浏览器预览下无 Tauri 运行时, 拖拽订阅直接跳过
    if (!("__TAURI_INTERNALS__" in window)) return;
    getCurrentWebview()
      .onDragDropEvent((event) => {
        const payload = event.payload;
        if (payload.type === "enter" || payload.type === "over") {
          setDragging(true);
          setDragHover(hitPeer(payload.position));
        } else if (payload.type === "drop") {
          const fp = hitPeer(payload.position);
          setDragging(false);
          setDragHover(null);
          const peer = fp ? peersRef.current[fp] : undefined;
          if (peer && payload.paths.length > 0) {
            sendFilesRef.current(peer, payload.paths).catch(console.error);
          }
        } else {
          setDragging(false);
          setDragHover(null);
        }
      })
      .then((u) => {
        if (alive) unlisten = u;
        else u();
      });
    return () => {
      alive = false;
      unlisten?.();
    };
  }, []);

  // 系统窗口材质生效时启用半透明背景变量(index.css 的 [data-vibrancy] 覆盖块)
  useEffect(() => {
    if (!("__TAURI_INTERNALS__" in window)) return;
    api
      .windowEffectsActive()
      .then((on) => {
        if (on) document.documentElement.dataset.vibrancy = "1";
      })
      .catch(console.error);
  }, []);

  // 全局快捷键: Esc 关最上层弹窗 / mod+, 打开设置 / mod+W 隐藏窗口
  // (接收确认弹窗不响应 Esc —— 拒绝需要明确操作, 防误触)
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const mod = e.metaKey || e.ctrlKey;
      if (e.key === "Escape") {
        if (showSettings) setShowSettings(false);
        else if (pinRetry) setPinRetry(null);
        else if (activePeer) setActivePeer(null);
      } else if (mod && e.key === ",") {
        e.preventDefault();
        setShowSettings(true);
      } else if (mod && e.key.toLowerCase() === "w") {
        // macOS 的 Cmd+W 走系统菜单 close(已被拦截转隐藏);
        // Windows/Linux 无系统默认, 这里补齐同样的"隐入托盘"语义
        e.preventDefault();
        if ("__TAURI_INTERNALS__" in window) {
          getCurrentWindow().hide().catch(console.error);
        }
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [showSettings, pinRetry, activePeer]);

  const self = dm.self;
  // 派生数组与回调保持引用稳定, 配合子组件 memo 隔离传输高频更新
  const peerList = useMemo(() => Object.values(dm.peers), [dm.peers]);
  const transferList = useMemo(() => Object.values(dm.transfers), [dm.transfers]);
  const toggleTheme = useCallback(() => setTheme((t) => (t === "dark" ? "light" : "dark")), []);
  const openSettings = useCallback(() => setShowSettings(true), []);
  /** 取头像图片 URL(非图片头像或未就绪时为 undefined) */
  const srcOf = (avatar: string | null | undefined) => {
    const hash = avatarHashOf(avatar);
    return hash ? dm.avatarSrcs[hash] : undefined;
  };

  return (
    <div className="flex h-full flex-col">
      <Header
        self={self}
        peerCount={peerList.length}
        theme={theme}
        onToggleTheme={toggleTheme}
        onOpenSettings={openSettings}
      />

      <div className="flex min-h-0 flex-1">
        <main className="relative min-w-0 flex-1">
          <Radar
            self={self}
            peers={peerList}
            avatarSrcs={dm.avatarSrcs}
            dragging={dragging}
            dragHover={dragHover}
            onPeerClick={setActivePeer}
          />
          {/* 拖拽中的引导提示(压在常驻扫描胶囊上方, pointer-events-none 不挡命中检测) */}
          {dragging && (
            <div className="pointer-events-none absolute inset-x-0 bottom-20 text-center">
              <span className="rounded-full border border-ember/60 bg-panel px-4 py-1.5 text-sm text-ember">
                {dragHover ? "松开即发送" : "拖到目标设备上发送"}
              </span>
            </div>
          )}
        </main>
        <aside className="w-80 shrink-0 border-l border-line bg-panel transition-colors duration-300">
          <TransferPanel
            transfers={transferList}
            texts={dm.texts}
            peers={peerList}
            getPin={dm.getPin}
            onPinLearned={dm.rememberPin}
            onTextSent={dm.addSentText}
            onSendImage={dm.sendClipboardImage}
            onPinRetry={setPinRetry}
          />
        </aside>
      </div>

      {/* 弹窗: 接收确认按到达顺序排队, 一次只显示一个 */}
      {dm.offers[0] && (
        <OfferModal
          offer={dm.offers[0]}
          avatarSrc={srcOf(dm.offers[0].peerAvatar)}
          onRespond={dm.respondOffer}
        />
      )}
      {activePeer && (
        <PeerActionModal
          peer={activePeer}
          avatarSrc={srcOf(activePeer.avatar)}
          getPin={dm.getPin}
          onPinLearned={dm.rememberPin}
          onSendFiles={(peer, paths) => {
            dm.sendFiles(peer, paths).catch(console.error);
          }}
          onSendImage={dm.sendClipboardImage}
          onClose={() => setActivePeer(null)}
        />
      )}
      {pinRetry && (
        <PinModal
          peerName={pinRetry.peerName}
          onSubmit={(pin) => {
            api.retrySend(pinRetry.transferId, pin).catch(console.error);
            // 重试成功与否由后续事件更新条目; 先记缓存, 再拒会重新弹
            if (pinRetry.peerFingerprint) dm.rememberPin(pinRetry.peerFingerprint, pin);
            setPinRetry(null);
          }}
          onClose={() => setPinRetry(null)}
        />
      )}
      {showSettings && self && (
        <SettingsModal
          fingerprint={self.fingerprint}
          // 昵称/头像/下载目录即时生效, 保存后刷新本机展示(含新头像加载)
          onSaved={dm.refreshSelf}
          onClose={() => setShowSettings(false)}
        />
      )}
    </div>
  );
}
