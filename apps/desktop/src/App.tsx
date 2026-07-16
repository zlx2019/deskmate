// deskmate 主界面: 顶栏 + 雷达 + 传输面板, 以及弹窗与全局文件拖拽

import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useDeskmate } from "./hooks/useDeskmate";
import { useI18n } from "./i18n";
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

/** 主题偏好: system 跟随操作系统, 其余为显式指定 */
type ThemePref = "system" | "dark" | "light";

/** 读取系统当前的明暗偏好(无 matchMedia 的环境按暗色) */
function systemTheme(): "dark" | "light" {
  return window.matchMedia?.("(prefers-color-scheme: light)").matches ? "light" : "dark";
}

/** 顶栏(设计稿风: 方块 logo + 在线数 + 端口 badge + 主题切换 + 设置)
 * memo: 传输高频更新时 props 未变即跳过 */
const Header = memo(function Header({
  self,
  peerCount,
  themePref,
  onToggleTheme,
  onOpenSettings,
}: {
  self: SelfInfoDto | null;
  peerCount: number;
  themePref: ThemePref;
  onToggleTheme: () => void;
  onOpenSettings: () => void;
}) {
  const { t } = useI18n();
  // "N 台设备在线" 数字加粗: 文案按数字劈开两段, 中英文语序不同也能各自成立
  const [onlineBefore, onlineAfter] = t.header.online(peerCount).split(String(peerCount));
  return (
    <header className="flex h-13 shrink-0 items-center gap-3 border-b border-line bg-panel px-4 transition-colors duration-300">
      <div className="flex items-center gap-2 text-[15px] font-medium tracking-[0.06em] text-fog">
        <img src={logoUrl} alt="" className="size-6" draggable={false} />
        Deskmate
      </div>
      <div className="ml-auto flex items-center gap-3">
        <span className="flex items-center gap-1.5 text-[13px] text-mist">
          <span className="size-2 rounded-full bg-live" />
          {onlineBefore}
          <span className="font-medium text-fog">{peerCount}</span>
          {onlineAfter}
        </span>
        <span className="rounded-md border border-line px-2 py-1 text-xs text-faint">
          {t.header.port(self?.port ?? "…")}
        </span>
        <button
          onClick={onToggleTheme}
          title={
            themePref === "system"
              ? t.header.toLight
              : themePref === "light"
                ? t.header.toDark
                : t.header.toSystem
          }
          className="flex size-8 cursor-pointer items-center justify-center rounded-lg border border-line text-fog transition-colors hover:border-line-2"
        >
          {/* 图标表示点击后的去向: 跟随系统→亮色→暗色→跟随系统 */}
          {themePref === "system" ? (
            /* 太阳: 点击切到亮色 */
            <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round">
              <circle cx="12" cy="12" r="4" />
              <path d="M12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4" />
            </svg>
          ) : themePref === "light" ? (
            /* 月亮: 点击切到暗色 */
            <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round">
              <path d="M21 12.8A9 9 0 1 1 11.2 3a7 7 0 0 0 9.8 9.8z" />
            </svg>
          ) : (
            /* 显示器: 点击回到跟随系统 */
            <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round">
              <rect x="2" y="3" width="20" height="14" rx="2" />
              <path d="M8 21h8M12 17v4" />
            </svg>
          )}
        </button>
        <button
          onClick={onOpenSettings}
          title={t.header.settings}
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
  const { t } = useI18n();
  const dm = useDeskmate();
  const [activePeer, setActivePeer] = useState<PeerDto | null>(null);
  const [showSettings, setShowSettings] = useState(false);
  // 待输入 PIN 重试的被拒任务
  const [pinRetry, setPinRetry] = useState<TransferItem | null>(null);
  // 主题偏好: 跟随系统 / 亮 / 暗三态, localStorage 持久化;
  // 未设置过的默认跟随系统, 老用户存过的显式值保持不变
  const [themePref, setThemePref] = useState<ThemePref>(() => {
    const saved = localStorage.getItem("dm-theme");
    return saved === "light" || saved === "dark" ? saved : "system";
  });
  // 系统明暗(仅"跟随系统"偏好时参与渲染; 监听常驻, 系统切换实时生效)
  const [sysTheme, setSysTheme] = useState<"dark" | "light">(systemTheme);
  useEffect(() => {
    const mq = window.matchMedia?.("(prefers-color-scheme: light)");
    if (!mq) return;
    const onChange = () => setSysTheme(systemTheme());
    mq.addEventListener("change", onChange);
    return () => mq.removeEventListener("change", onChange);
  }, []);
  const theme = themePref === "system" ? sysTheme : themePref;

  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    localStorage.setItem("dm-theme", themePref);
    // 原生窗口 chrome(标题栏/边框)与内容主题同步(Windows 上差异明显);
    // 跟随系统时把 chrome 也交还系统(null), 避免系统切换后 chrome 滞留
    if ("__TAURI_INTERNALS__" in window) {
      getCurrentWindow()
        .setTheme(themePref === "system" ? null : themePref)
        .catch(console.error);
    }
  }, [theme, themePref]);
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
  // 三态循环: 跟随系统 → 亮 → 暗 → 跟随系统
  const toggleTheme = useCallback(
    () => setThemePref((p) => (p === "system" ? "light" : p === "light" ? "dark" : "system")),
    [],
  );
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
        themePref={themePref}
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
                {dragHover ? t.radar.dropToSend : t.radar.dragToTarget}
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
            onPause={dm.pauseTransfer}
            onResume={dm.resumeTransfer}
            onPinLearned={dm.rememberPin}
            onTextSent={dm.addSentText}
            onSendImage={dm.sendClipboardImage}
            onRemoveText={dm.removeText}
            onClearTexts={dm.clearTexts}
            onPinRetry={setPinRetry}
          />
        </aside>
      </div>

      {/* 弹窗: 接收确认按到达顺序排队, 一次只显示一个
          key 按 offerId 强制重挂载: 否则队首从 A 换 B 时 React 复用实例,
          A 的保存目录/冲突选择会泄漏给 B */}
      {dm.offers[0] && (
        <OfferModal
          key={dm.offers[0].offerId}
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
