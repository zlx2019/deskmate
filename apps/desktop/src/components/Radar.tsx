// Discovery view with a dotted texture, island map, center pulses, and peer bubbles.
// Light and dark themes switch through CSS variables.
//
// Peers pop in with a landing ripple, bob while idle, and shrink out after
// useExitingPeers briefly retains removed data.

import { memo, useEffect, useMemo, useRef, useState } from "react";
import { Typewriter as ATypewriter } from "animal-island-ui";
import { useI18n } from "../i18n";
import { avatarHashOf, type PeerDto, type SelfInfoDto } from "../types";

/** Derives a stable avatar hue from a fingerprint. */
function hueOf(fingerprint: string): number {
  return parseInt(fingerprint.slice(0, 4) || "0", 16) % 360;
}

/** Returns the first Unicode character of a display name. */
function initialOf(name: string): string {
  return [...name][0]?.toUpperCase() ?? "?";
}

/** Circular avatar with image, emoji, and initial fallbacks. */
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
  /** Blob URL supplied when an image avatar is cached. */
  src?: string | null;
}) {
  const hue = hueOf(fingerprint);
  // Peer-advertised emoji is untrusted; limit it to four code points and hide img markers.
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

/** Organic island map with dotted texture, corner islands, distance rings,
 * curved roads, mountains, lakes, trees, and clouds. The viewBox matches the
 * near-square desktop window and aligns ring center (330,294) with the local
 * node at container position 50%,46%. */
const MapBackdrop = memo(function MapBackdrop() {
  return (
    <svg
      className="absolute inset-0 h-full w-full"
      viewBox="0 0 660 640"
      preserveAspectRatio="xMidYMid slice"
      aria-hidden
    >
      <defs>
        <pattern id="dm-dots" width="26" height="26" patternUnits="userSpaceOnUse">
          <circle cx="2" cy="2" r="1.6" fill="var(--color-dots)" />
        </pattern>
      </defs>
      <rect width="660" height="640" fill="url(#dm-dots)" />
      {/* Layered mountains with a snow-capped main peak. */}
      <path d="M 262 122 Q 310 50 358 122 Z" fill="var(--color-hill-2)" />
      <path d="M 185 122 Q 242 26 300 122 Z" fill="var(--color-hill)" />
      <path d="M 225 74 Q 242 44 260 74 Q 242 84 225 74 Z" fill="rgba(255,255,255,0.85)" />
      {/* Organic island shapes with a light beach-like outline. */}
      <path
        d="M-50 90 C 60 30, 190 90, 175 190 C 160 270, 40 300, -50 250 Z"
        fill="var(--color-isle)"
        stroke="var(--color-road)"
        strokeWidth="5"
      />
      <path
        d="M430 -40 C 570 -20, 660 60, 620 150 C 580 230, 460 210, 425 135 C 400 80, 395 5, 430 -40 Z"
        fill="var(--color-isle)"
        stroke="var(--color-road)"
        strokeWidth="5"
      />
      <path
        d="M470 400 C 590 360, 700 430, 685 530 C 665 625, 500 625, 455 530 C 432 475, 435 430, 470 400 Z"
        fill="var(--color-isle-2)"
        stroke="var(--color-road)"
        strokeWidth="5"
      />
      <path
        d="M100 450 C 190 400, 300 450, 290 530 C 278 615, 110 615, 75 545 C 58 505, 65 475, 100 450 Z"
        fill="var(--color-isle-2)"
        stroke="var(--color-road)"
        strokeWidth="5"
      />
      {/* Dashed elliptical distance rings. */}
      <ellipse
        cx="330"
        cy="294"
        rx="170"
        ry="150"
        fill="none"
        stroke="var(--color-road)"
        strokeWidth="1"
        strokeDasharray="3 6"
      />
      <ellipse
        cx="330"
        cy="294"
        rx="255"
        ry="225"
        fill="none"
        stroke="var(--color-road)"
        strokeWidth="1"
        strokeDasharray="3 6"
      />
      {/* Curved roads. */}
      <path
        d="M-20 400 C 140 340, 240 410, 330 370 C 430 325, 520 370, 690 300"
        fill="none"
        stroke="var(--color-road)"
        strokeWidth="3"
        strokeLinecap="round"
      />
      <path
        d="M210 -20 C 240 100, 180 220, 265 320 C 335 405, 330 480, 355 660"
        fill="none"
        stroke="var(--color-road)"
        strokeWidth="3"
        strokeLinecap="round"
      />
      {/* Organic lakes with light shores and highlights at both edges. */}
      <g>
        <path
          d="M-30 360 C 0 338, 70 342, 95 368 C 118 392, 100 420, 60 424 C 15 428, -25 415, -30 390 Z"
          fill="var(--color-water)"
          stroke="var(--color-road)"
          strokeWidth="4"
        />
        <ellipse cx="38" cy="384" rx="22" ry="7" fill="rgba(255,255,255,0.35)" />
        <path d="M 62 402 q 11 -5 22 0" fill="none" stroke="rgba(255,255,255,0.5)" strokeWidth="1.5" strokeLinecap="round" />
        <path
          d="M 553 290 C 567 270, 609 266, 633 286 C 655 304, 645 330, 611 336 C 577 342, 549 318, 553 290 Z"
          fill="var(--color-water)"
          stroke="var(--color-road)"
          strokeWidth="4"
        />
        <ellipse cx="587" cy="298" rx="16" ry="6" fill="rgba(255,255,255,0.35)" />
      </g>
      {/* Groves and scattered trees with minimal round canopies. */}
      <g>
        {[
          // Upper-left island grove.
          [70, 120],
          [98, 140],
          [126, 162],
          [60, 170],
          [88, 190],
          [140, 220],
          // Mountain base and upper clearing.
          [340, 132],
          [368, 100],
          // Upper-right island.
          [552, 62],
          [582, 130],
          [508, 172],
          // Left lakeshore.
          [112, 348],
          [86, 442],
          // Right lakeshore.
          [584, 352],
          // Lower-right island grove.
          [545, 468],
          [575, 508],
          [610, 478],
          [560, 545],
          [606, 532],
          // Lower-left island.
          [110, 500],
          [148, 532],
          [235, 555],
          // Lower clearing.
          [355, 588],
          [398, 598],
          [424, 622],
        ].map(([cx, cy]) => (
          <g key={`${cx}-${cy}`}>
            <rect x={cx - 1.5} y={cy} width="3" height="8" rx="1.5" fill="var(--color-trunk)" />
            <circle cx={cx} cy={cy - 4} r="7" fill="var(--color-tree)" />
            <circle cx={cx - 4} cy={cy - 1} r="4.5" fill="var(--color-tree)" />
            <circle cx={cx + 4} cy={cy - 1} r="4.5" fill="var(--color-tree)" />
          </g>
        ))}
      </g>
      {/* Dark-theme moon and stars, hidden by CSS in the light theme. */}
      <g className="map-night">
        <circle cx="388" cy="52" r="24" fill="none" stroke="rgba(242,232,201,0.22)" strokeWidth="5" />
        <circle cx="388" cy="52" r="17" fill="#f2e8c9" opacity="0.92" />
        <circle cx="382" cy="46" r="3.2" fill="rgba(0,0,0,0.09)" />
        <circle cx="394" cy="57" r="2.3" fill="rgba(0,0,0,0.08)" />
        {/* Two four-point stars and scattered smaller stars. */}
        {(
          [
            [210, 78, 6],
            [572, 178, 5],
          ] as const
        ).map(([x, y, s]) => (
          <path
            key={`star-${x}-${y}`}
            d={`M ${x} ${y - s} L ${x + s * 0.28} ${y - s * 0.28} L ${x + s} ${y} L ${x + s * 0.28} ${y + s * 0.28} L ${x} ${y + s} L ${x - s * 0.28} ${y + s * 0.28} L ${x - s} ${y} L ${x - s * 0.28} ${y - s * 0.28} Z`}
            fill="rgba(242,232,201,0.8)"
          />
        ))}
        {(
          [
            [82, 62, 1.7],
            [152, 96, 1.3],
            [244, 42, 1.5],
            [318, 132, 1.2],
            [458, 32, 1.6],
            [502, 122, 1.3],
            [612, 68, 1.7],
            [76, 296, 1.3],
            [622, 250, 1.4],
            [345, 218, 1.1],
          ] as const
        ).map(([x, y, r]) => (
          <circle key={`dot-${x}-${y}`} cx={x} cy={y} r={r} fill="rgba(255,255,255,0.65)" />
        ))}
      </g>

      {/* Slowly drifting clouds with offset timing and dark-theme dimming. */}
      <g className="anim-cloud" fill="rgba(255,255,255,0.75)">
        <ellipse cx="150" cy="330" rx="34" ry="13" />
        <ellipse cx="175" cy="320" rx="22" ry="11" />
      </g>
      <g className="anim-cloud-slow" fill="rgba(255,255,255,0.6)">
        <ellipse cx="540" cy="280" rx="28" ry="11" />
        <ellipse cx="562" cy="272" rx="18" ry="9" />
      </g>
      <g className="anim-cloud" style={{ animationDelay: "-9s" }} fill="rgba(255,255,255,0.7)">
        <ellipse cx="430" cy="52" rx="30" ry="12" />
        <ellipse cx="452" cy="43" rx="19" ry="10" />
      </g>
    </svg>
  );
});

/** Rotates footer hints through a typewriter animation and timed pause. */
function RotatingTips() {
  const { t } = useI18n();
  const tips = [t.radar.scanning, t.radar.dragHint];
  const [idx, setIdx] = useState(0);
  const timerRef = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  // Clear the hold timer when switching or unmounting.
  useEffect(() => () => clearTimeout(timerRef.current), [idx]);
  return (
    <span className="whitespace-nowrap">
      <ATypewriter
        trigger={idx}
        speed={80}
        onDone={() => {
          // A trigger change can fire onDone for stale progress. Replacing the
          // timer guarantees only the latest completion schedules a switch.
          clearTimeout(timerRef.current);
          timerRef.current = setTimeout(() => setIdx((i) => (i + 1) % tips.length), 3200);
        }}
      >
        {tips[idx]}
      </ATypewriter>
    </span>
  );
}

/** Announces peer arrival or departure through a temporary top typewriter.
 * Initial discoveries during the startup grace period are not announced. */
function PresenceToast({ peers }: { peers: PeerDto[] }) {
  const { t } = useI18n();
  // seq forces replay when a same-named peer repeatedly reconnects.
  const [msg, setMsg] = useState<{ name: string; online: boolean; seq: number } | null>(null);
  // Preserve fingerprint-to-name snapshots for departure announcements.
  const seenRef = useRef<Map<string, string> | null>(null);
  const mountAtRef = useRef(Date.now());
  const seqRef = useRef(0);
  const timerRef = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);

  useEffect(() => {
    const cur = new Map(peers.map((p) => [p.fingerprint, p.name]));
    // The first pass records a snapshot; later set differences find joins and leaves.
    if (seenRef.current === null) {
      seenRef.current = cur;
      return;
    }
    const prev = seenRef.current;
    seenRef.current = cur;
    const joined = peers.filter((p) => !prev.has(p.fingerprint));
    const left = [...prev].filter(([fp]) => !cur.has(fp)).map(([, name]) => name);
    if (Date.now() - mountAtRef.current < 3000) return;
    // Prefer a join when both occur in the same render.
    const item =
      joined.length > 0
        ? { name: joined[joined.length - 1].name, online: true }
        : left.length > 0
          ? { name: left[left.length - 1], online: false }
          : null;
    if (!item) return;
    seqRef.current += 1;
    setMsg({ ...item, seq: seqRef.current });
    clearTimeout(timerRef.current);
    timerRef.current = setTimeout(() => setMsg(null), 4500);
  }, [peers]);
  useEffect(() => () => clearTimeout(timerRef.current), []);

  if (!msg) return null;
  return (
    <div className="pointer-events-none absolute inset-x-0 top-4 z-10 flex justify-center">
      <div className="anim-fade-up flex items-center gap-2 rounded-full border-2 border-line bg-panel px-4 py-1.5 text-[13px] font-bold text-fog shadow-[0_2px_0_rgba(41,71,51,0.12)]">
        <span
          className={`anim-breathe inline-block size-2 rounded-full ${msg.online ? "bg-live" : "bg-faint"}`}
        />
        <span className="whitespace-nowrap">
          <ATypewriter trigger={msg.seq} speed={70}>
            {msg.online ? t.radar.peerJoined(msg.name) : t.radar.peerLeft(msg.name)}
          </ATypewriter>
        </span>
      </div>
    </div>
  );
}

/** Rendered peer retained briefly after removal for its exit animation. */
interface RenderedPeer {
  peer: PeerDto;
  /** Whether the departure animation is running. */
  leaving: boolean;
}

/** Exit retention duration, slightly longer than .anim-pop-out. */
const LEAVE_MS = 420;

/** Maintains peers with delayed removal for entry and exit animations. */
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
          // Reuse unchanged objects for online peers or peers that returned mid-exit.
          next.push(cur === r.peer && !r.leaving ? r : { peer: cur, leaving: false });
          seen.add(cur.fingerprint);
        } else {
          // Retain newly offline and exiting peers until the animation completes.
          next.push(r.leaving ? r : { ...r, leaving: true });
        }
      }
      for (const p of peers) {
        if (!seen.has(p.fingerprint)) next.push({ peer: p, leaving: false });
      }
      // Preserve the array reference when nothing changed so React can skip the update.
      if (next.length === prev.length && next.every((r, i) => r === prev[i])) {
        return prev;
      }
      return next;
    });
  }, [peers]);

  // Remove peers after exit animation. Depend on the leaving-set signature so
  // frequent transfer renders cannot reset timers and retain offline peers forever.
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
  /** Image-avatar Blob URLs keyed by hash. */
  avatarSrcs: Record<string, string>;
  /** Fingerprint of the drag-hover target. */
  dragHover: string | null;
  /** Whether a file drag is active. */
  dragging: boolean;
  onPeerClick: (peer: PeerDto) => void;
}

/** Memoized discovery view isolated from high-frequency transfer updates. */
export const Radar = memo(function Radar({
  self,
  peers,
  avatarSrcs,
  dragHover,
  dragging,
  onPeerClick,
}: RadarProps) {
  const { t } = useI18n();
  const rendered = useExitingPeers(peers);

  /** Returns an avatar image URL, or undefined when unavailable. */
  const srcOf = (avatar: string | null | undefined) => {
    const hash = avatarHashOf(avatar);
    return hash ? avatarSrcs[hash] : undefined;
  };

  // Sort peers by fingerprint for stable circular placement, then add a small
  // fingerprint-based offset. CSS transitions smooth later position changes.
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
      <PresenceToast peers={peers} />

      {/* Center: local device with two offset pulse rings. */}
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
        <div className="mx-auto mt-2 w-fit max-w-44 truncate rounded-full border-2 border-line bg-panel px-3 py-0.5 text-xs font-bold text-fog shadow-[0_2px_0_rgba(41,71,51,0.12)]">
          {self ? `${self.name} · ${t.radar.myDevice}` : "…"}
        </div>
        <div className="mt-1 text-[11px] tracking-[0.18em] text-white/80">{t.radar.thisDevice}</div>
      </div>

      {/* Nearby peer bubbles. */}
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
              {/* Arrival ring expands once and remains transparent afterward. */}
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
                {/* Offset idle bobbing. */}
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
              <div className="mx-auto mt-2 w-fit max-w-32 truncate rounded-full border-2 border-line bg-panel px-3 py-0.5 text-xs font-bold text-fog shadow-[0_2px_0_rgba(41,71,51,0.12)]">
                {peer.name}
              </div>
            </div>
          </button>
        );
      })}

      {/* Footer with persistent scan status and drag guidance. */}
      <div className="pointer-events-none absolute inset-x-0 bottom-4 flex flex-col items-center">
        <div className="flex items-center gap-2 rounded-full border-2 border-line bg-panel px-4 py-1.5 text-[13px] font-bold text-fog/80 shadow-[0_2px_0_rgba(41,71,51,0.12)]">
          <span className="anim-breathe inline-block size-2 rounded-full bg-sonar" />
          {/* Online count shares the capsule; empty state shows only rotating hints. */}
          {peers.length > 0 && (
            <>
              <span className="text-fog">{t.radar.online(peers.length)}</span>
              <span className="text-mist/60">·</span>
            </>
          )}
          {/* Typewriter rotation for discovery and drag hints. */}
          <RotatingTips />
        </div>
      </div>
    </div>
  );
});
