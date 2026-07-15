// 搜寻界面: 点阵底纹 + 有机岛屿地图 + 中心脉冲涟漪 + 附近设备气泡
// (视觉对齐 deskmate_map_ui_redesign_v3 设计稿, 亮暗主题经 CSS 变量切换)
//
// 节点上线冒泡弹跳(pop-in + 落点涟漪), 待机轻浮动(bob),
// 下线收缩消散(pop-out, 由 useExitingPeers 在数据移除后保留一拍播放)。

import { memo, useEffect, useMemo, useState } from "react";
import { avatarHashOf, type PeerDto, type SelfInfoDto } from "../types";

/** 从指纹取稳定色相(节点头像配色) */
function hueOf(fingerprint: string): number {
  return parseInt(fingerprint.slice(0, 4) || "0", 16) % 360;
}

/** 名称首字符(中文取第一个字) */
function initialOf(name: string): string {
  return [...name][0]?.toUpperCase() ?? "?";
}

/** 圆形头像(节点与本机通用): 图片 > emoji > 首字母 三级回退 */
export function Avatar({
  name,
  fingerprint,
  size,
  avatar,
  src,
}: {
  name: string;
  fingerprint: string;
  size: number;
  avatar?: string | null;
  /** 图片头像的 Blob URL(avatar 为 img: 且缓存就绪时提供) */
  src?: string | null;
}) {
  const hue = hueOf(fingerprint);
  // emoji 来自对端广播不可信: 截前 4 个 code point 防超长破版; img: 标记不作 emoji 展示
  const emoji =
    avatar && !avatar.startsWith("img:") ? [...avatar].slice(0, 4).join("") : null;
  return (
    <div
      className="flex items-center justify-center overflow-hidden rounded-full font-medium text-white"
      style={{
        width: size,
        height: size,
        fontSize: size * (emoji ? 0.5 : 0.4),
        background: `linear-gradient(135deg, hsl(${hue} 55% 55%), hsl(${(hue + 36) % 360} 52% 42%))`,
      }}
    >
      {src ? (
        <img src={src} alt="" className="size-full object-cover" draggable={false} />
      ) : (
        <span style={{ lineHeight: 1 }}>{emoji ?? initialOf(name)}</span>
      )}
    </div>
  );
}

/** 有机岛屿地图(设计稿同款: 点阵底纹 + 岛屿地块 + 虚线距离环 + 曲线道路) */
const MapBackdrop = memo(function MapBackdrop() {
  return (
    <svg
      className="absolute inset-0 h-full w-full"
      viewBox="0 0 660 470"
      preserveAspectRatio="xMidYMid slice"
      aria-hidden
    >
      <defs>
        <pattern id="dm-dots" width="26" height="26" patternUnits="userSpaceOnUse">
          <circle cx="2" cy="2" r="1.6" fill="var(--color-dots)" />
        </pattern>
      </defs>
      <rect width="660" height="470" fill="url(#dm-dots)" />
      {/* 岛屿地块: 有机 blob 形状 */}
      <path
        d="M-40 120 C 80 60, 180 150, 150 230 C 130 300, 20 320, -40 280 Z"
        fill="var(--color-isle)"
      />
      <path
        d="M420 -30 C 560 -10, 640 70, 600 150 C 560 220, 440 200, 410 130 C 390 80, 380 10, 420 -30 Z"
        fill="var(--color-isle)"
      />
      <path
        d="M480 320 C 590 290, 700 350, 680 440 C 660 520, 500 520, 460 440 C 440 390, 440 340, 480 320 Z"
        fill="var(--color-isle-2)"
      />
      <path
        d="M120 380 C 200 340, 300 380, 290 450 C 280 520, 120 520, 90 460 C 75 425, 85 398, 120 380 Z"
        fill="var(--color-isle-2)"
      />
      {/* 距离环: 虚线椭圆 */}
      <ellipse
        cx="330"
        cy="235"
        rx="150"
        ry="120"
        fill="none"
        stroke="var(--color-road)"
        strokeWidth="1"
        strokeDasharray="3 6"
      />
      <ellipse
        cx="330"
        cy="235"
        rx="235"
        ry="185"
        fill="none"
        stroke="var(--color-road)"
        strokeWidth="1"
        strokeDasharray="3 6"
      />
      {/* 曲线道路 */}
      <path
        d="M-20 330 C 140 280, 240 340, 330 300 C 430 255, 520 300, 690 240"
        fill="none"
        stroke="var(--color-road)"
        strokeWidth="3"
        strokeLinecap="round"
      />
      <path
        d="M200 -20 C 230 90, 180 180, 260 260 C 330 330, 320 400, 350 500"
        fill="none"
        stroke="var(--color-road)"
        strokeWidth="3"
        strokeLinecap="round"
      />
      {/* 建筑点群 */}
      <g fill="var(--color-road)">
        <circle cx="90" cy="180" r="3" />
        <circle cx="110" cy="205" r="3" />
        <circle cx="75" cy="225" r="3" />
        <circle cx="520" cy="90" r="3" />
        <circle cx="548" cy="115" r="3" />
        <circle cx="560" cy="390" r="3" />
        <circle cx="585" cy="415" r="3" />
        <circle cx="170" cy="430" r="3" />
        <circle cx="200" cy="455" r="3" />
      </g>
    </svg>
  );
});

/** 渲染节点: 数据移除后保留一拍播放退场动画 */
interface RenderedPeer {
  peer: PeerDto;
  /** 正在播放下线消散动画 */
  leaving: boolean;
}

/** 退场时长(与 .anim-pop-out 对齐, 略长保证播完) */
const LEAVE_MS = 420;

/** 维护带退场态的节点列表: 上线即入列(pop-in), 下线标记 leaving 延迟移除 */
function useExitingPeers(peers: PeerDto[]): RenderedPeer[] {
  const [rendered, setRendered] = useState<RenderedPeer[]>([]);

  useEffect(() => {
    setRendered((prev) => {
      const live = new Map(peers.map((p) => [p.fingerprint, p]));
      const seen = new Set<string>();
      const next: RenderedPeer[] = [];
      for (const r of prev) {
        const cur = live.get(r.peer.fingerprint);
        if (cur) {
          // 仍在线(或下线动画未播完时又回来了): 数据未变则复用原对象
          next.push(cur === r.peer && !r.leaving ? r : { peer: cur, leaving: false });
          seen.add(cur.fingerprint);
        } else {
          // 刚下线或退场中: 保留播放消散动画
          next.push(r.leaving ? r : { ...r, leaving: true });
        }
      }
      for (const p of peers) {
        if (!seen.has(p.fingerprint)) next.push({ peer: p, leaving: false });
      }
      // 无实质变化时返回原引用, 让 React 跳过本次更新(防上游重渲引发的空转)
      if (next.length === prev.length && next.every((r, i) => r === prev[i])) {
        return prev;
      }
      return next;
    });
  }, [peers]);

  // 退场动画播完后真正移除。依赖取 leaving 集合的内容签名而非数组引用:
  // 传输进行中的高频重渲不会重置定时器, 否则渲染间隔 < LEAVE_MS 时
  // 离线节点会因定时器被反复重建而永远留在屏上。
  const leavingKeys = rendered
    .filter((r) => r.leaving)
    .map((r) => r.peer.fingerprint)
    .join(",");
  useEffect(() => {
    if (!leavingKeys) return;
    const timer = setTimeout(
      () => setRendered((prev) => prev.filter((r) => !r.leaving)),
      LEAVE_MS,
    );
    return () => clearTimeout(timer);
  }, [leavingKeys]);

  return rendered;
}

interface RadarProps {
  self: SelfInfoDto | null;
  peers: PeerDto[];
  /** 图片头像 Blob URL 映射(hash → url) */
  avatarSrcs: Record<string, string>;
  /** 拖拽悬停的节点指纹(高亮虹吸态) */
  dragHover: string | null;
  /** 是否处于文件拖拽中 */
  dragging: boolean;
  onPeerClick: (peer: PeerDto) => void;
}

/** 搜寻主视图(memo: 传输面板高频更新时 props 未变即整体跳过) */
export const Radar = memo(function Radar({
  self,
  peers,
  avatarSrcs,
  dragHover,
  dragging,
  onPeerClick,
}: RadarProps) {
  const rendered = useExitingPeers(peers);

  /** 取头像图片 URL(非图片头像或未就绪时为 undefined) */
  const srcOf = (avatar: string | null | undefined) => {
    const hash = avatarHashOf(avatar);
    return hash ? avatarSrcs[hash] : undefined;
  };

  // 节点按指纹排序后均匀分布圆周(位置稳定不跳变), 指纹微偏打散;
  // 顶部起始(-90°), 位置变化经 CSS transition 平滑过渡
  const positioned = useMemo(() => {
    const sorted = [...rendered].sort((a, b) =>
      a.peer.fingerprint.localeCompare(b.peer.fingerprint),
    );
    return sorted.map((r, i) => {
      const fp = r.peer.fingerprint;
      const jitter = ((parseInt(fp.slice(4, 8) || "0", 16) % 100) / 100 - 0.5) * 0.5;
      const angle = -Math.PI / 2 + (i / sorted.length) * Math.PI * 2 + jitter;
      const radius = 26 + (parseInt(fp.slice(8, 10) || "0", 16) % 12);
      return {
        ...r,
        x: 50 + radius * Math.cos(angle),
        y: 46 + radius * Math.sin(angle) * 0.92,
      };
    });
  }, [rendered]);

  return (
    <div className="relative h-full overflow-hidden bg-map transition-colors duration-300">
      <MapBackdrop />

      {/* 中心: 本机 + 双圈错峰脉冲涟漪 */}
      <div className="absolute left-1/2 top-[46%] -translate-x-1/2 -translate-y-1/2 text-center">
        <div className="relative inline-block">
          {[0, 1.2].map((delay) => (
            <span
              key={delay}
              className="anim-sonar-wave pointer-events-none absolute -inset-0.5 rounded-full border-2 border-sonar"
              style={{ animationDelay: `${delay}s` }}
            />
          ))}
          <div className="relative overflow-hidden rounded-full border-[3px] border-panel-2">
            {self ? (
              <Avatar
                name={self.name}
                fingerprint={self.fingerprint}
                size={56}
                avatar={self.avatar}
                src={srcOf(self.avatar)}
              />
            ) : (
              <div className="size-14 rounded-full bg-sonar-dim" />
            )}
          </div>
        </div>
        <div className="mx-auto mt-2 w-fit max-w-44 truncate rounded-full border border-line bg-panel px-2.5 py-0.5 text-xs text-fog">
          {self ? `${self.name} · 我的设备` : "…"}
        </div>
        <div className="mt-1 text-[11px] tracking-[0.18em] text-faint">THIS DEVICE</div>
      </div>

      {/* 附近设备气泡 */}
      {positioned.map(({ peer, leaving, x, y }, i) => {
        const hovered = dragHover === peer.fingerprint;
        return (
          <button
            key={peer.fingerprint}
            data-peer={peer.fingerprint}
            onClick={() => onPeerClick(peer)}
            disabled={leaving}
            className="absolute -translate-x-1/2 -translate-y-1/2 cursor-pointer text-center transition-[left,top] duration-500"
            style={{ left: `${x}%`, top: `${y}%` }}
            title={`${peer.name} · ${peer.addrs[0] ?? ""}:${peer.port}`}
          >
            <div className={leaving ? "anim-pop-out" : "anim-pop-in"}>
              {/* 上线光环: 落点一圈爆开(动画 both 播完停在透明) */}
              {!leaving && (
                <span className="anim-ring-burst pointer-events-none absolute -inset-1 rounded-full border-2 border-sonar" />
              )}
              <div
                className={`relative inline-block transition-transform duration-200 ${
                  hovered ? "scale-125" : dragging ? "scale-110" : "hover:scale-110"
                }`}
              >
                {hovered && (
                  <>
                    <span className="anim-ping-ring absolute inset-0 rounded-full border-2 border-ember" />
                    <span className="absolute -inset-2 rounded-full border-2 border-dashed border-ember/80" />
                  </>
                )}
                {/* 待机轻浮动, 错峰起跳 */}
                <div
                  className="anim-bob overflow-hidden rounded-full border-[3px] border-panel-2"
                  style={{ animationDelay: `${(i % 5) * 0.6}s` }}
                >
                  <Avatar
                    name={peer.name}
                    fingerprint={peer.fingerprint}
                    size={48}
                    avatar={peer.avatar}
                    src={srcOf(peer.avatar)}
                  />
                </div>
              </div>
              <div className="mx-auto mt-2 w-fit max-w-32 truncate rounded-full border border-line bg-panel px-2.5 py-0.5 text-xs text-fog">
                {peer.name}
              </div>
            </div>
          </button>
        );
      })}

      {/* 底部: 常驻扫描状态 + 拖拽提示(设计稿同款) */}
      <div className="pointer-events-none absolute inset-x-0 bottom-4 flex flex-col items-center gap-2">
        <div className="flex items-center gap-2 rounded-full border border-line bg-panel px-4 py-1.5 text-[13px] text-mist">
          <span className="anim-breathe inline-block size-2 rounded-full bg-sonar" />
          <span className="scan-dots">正在搜寻附近的设备</span>
        </div>
        <div className="text-xs text-faint">拖拽文件到设备头像上即可发送</div>
      </div>
    </div>
  );
});
