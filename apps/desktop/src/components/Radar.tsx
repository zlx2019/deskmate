// Discovery view: an island where the local device stands at the center and
// peers land on random free spots around it, linked by dotted trails. Light and
// dark themes switch through --isle-* CSS variables.
//
// Peers pop in with a landing puff and shrink out after useExitingPeers briefly
// retains removed data.

import { memo, useEffect, useMemo, useRef, useState, type CSSProperties, type RefObject } from "react";
import { Typewriter as ATypewriter } from "animal-island-ui";
import { useI18n } from "../i18n";
import { avatarHashOf, type NoteFlight, type PeerDto, type SelfInfoDto } from "../types";
import { CX, CY, VIEW_H, VIEW_W } from "./map/geometry";
import { IslandBackdrop } from "./map/IslandBackdrop";
import { clearedProps, usePlacedPeers } from "./map/layout";
import { SCENE } from "./map/scene";
import { Trails, type TransferLinks } from "./map/Trails";

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

/** Uniform "meet" fit of the design space into an element. */
function useFit(ref: RefObject<HTMLElement | null>): { s: number; ox: number; oy: number } {
  const [fit, setFit] = useState({ s: 1, ox: 0, oy: 0 });
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const update = () => {
      const w = el.clientWidth;
      const h = el.clientHeight;
      const s = Math.min(w / VIEW_W, h / VIEW_H);
      setFit({ s, ox: (w - VIEW_W * s) / 2, oy: (h - VIEW_H * s) / 2 });
    };
    update();
    const observer = new ResizeObserver(update);
    observer.observe(el);
    return () => observer.disconnect();
  }, [ref]);
  return fit;
}

/** Tracks whether the page is hidden (window minimized to the tray). */
function usePageHidden(): boolean {
  const [hidden, setHidden] = useState(document.hidden);
  useEffect(() => {
    const onChange = () => setHidden(document.hidden);
    document.addEventListener("visibilitychange", onChange);
    return () => document.removeEventListener("visibilitychange", onChange);
  }, []);
  return hidden;
}

/** Tracks the system reduced-motion preference. */
function useReducedMotion(): boolean {
  const [reduced, setReduced] = useState(() => matchMedia("(prefers-reduced-motion: reduce)").matches);
  useEffect(() => {
    const mq = matchMedia("(prefers-reduced-motion: reduce)");
    const onChange = () => setReduced(mq.matches);
    mq.addEventListener("change", onChange);
    return () => mq.removeEventListener("change", onChange);
  }, []);
  return reduced;
}

/** Avatar border width; the avatar center sits this far below its box edge plus the radius. */
const AVATAR_BORDER = 3;
/** Local-device avatar diameter in design units. */
const SELF_SIZE = 62;

interface RadarProps {
  self: SelfInfoDto | null;
  peers: PeerDto[];
  /** Image-avatar Blob URLs keyed by hash. */
  avatarSrcs: Record<string, string>;
  /** Fingerprint of the drag-hover target. */
  dragHover: string | null;
  /** Whether a file drag is active. */
  dragging: boolean;
  /** Running transfers by peer; their dots flow along the trails. */
  links: TransferLinks;
  /** Text-message notes currently flying along trails. */
  flights: NoteFlight[];
  onPeerClick: (peer: PeerDto) => void;
}

/** Memoized discovery view isolated from high-frequency transfer updates. */
export const Radar = memo(function Radar({
  self,
  peers,
  avatarSrcs,
  dragHover,
  dragging,
  links,
  flights,
  onPeerClick,
}: RadarProps) {
  const { t } = useI18n();
  const rendered = useExitingPeers(peers);
  const rootRef = useRef<HTMLDivElement>(null);
  const sceneRef = useRef<SVGSVGElement>(null);
  const fxRef = useRef<SVGSVGElement>(null);
  const fit = useFit(rootRef);
  const hidden = usePageHidden();
  const reduced = useReducedMotion();
  const [hoverFp, setHoverFp] = useState<string | null>(null);

  const placed = usePlacedPeers(rendered);
  const cleared = useMemo(() => clearedProps(SCENE.props, placed), [placed]);
  const mailFlag = useMemo(() => [...links.values()].includes("recv"), [links]);
  const highlight = dragHover ?? hoverFp;

  // SMIL animations ignore CSS play state, so pause them through the SVG API:
  // everything while hidden, ambient scenery only under reduced motion.
  useEffect(() => {
    if (hidden || reduced) sceneRef.current?.pauseAnimations();
    else sceneRef.current?.unpauseAnimations();
    if (hidden) fxRef.current?.pauseAnimations();
    else fxRef.current?.unpauseAnimations();
  }, [hidden, reduced]);

  /** Returns an avatar image URL, or undefined when unavailable. */
  const srcOf = (avatar: string | null | undefined) => {
    const hash = avatarHashOf(avatar);
    return hash ? avatarSrcs[hash] : undefined;
  };

  // Nodes live in design units inside the fitted layer; counter-scale keeps
  // avatars readable (at least 75%) when the island shrinks in small windows.
  const nodeScale = Math.min(1, Math.max(0.75, fit.s)) / fit.s;
  /** Positions a node so its avatar center sits on (x, y). */
  const anchorAt = (x: number, y: number, size: number): CSSProperties => {
    const anchor = size / 2 + AVATAR_BORDER;
    return {
      left: x,
      top: y,
      transform: `translate(-50%, ${-anchor}px) scale(${nodeScale})`,
      transformOrigin: `50% ${anchor}px`,
    };
  };

  return (
    <div ref={rootRef} className="isle-map relative h-full overflow-hidden" data-paused={hidden ? "" : undefined}>
      <svg
        ref={sceneRef}
        className="isle-scene pointer-events-none absolute inset-0 h-full w-full overflow-visible"
        viewBox={`0 0 ${VIEW_W} ${VIEW_H}`}
        preserveAspectRatio="xMidYMid meet"
        aria-hidden
      >
        <IslandBackdrop cleared={cleared} mailFlag={mailFlag} />
      </svg>
      <svg
        ref={fxRef}
        className="pointer-events-none absolute inset-0 h-full w-full overflow-visible"
        viewBox={`0 0 ${VIEW_W} ${VIEW_H}`}
        preserveAspectRatio="xMidYMid meet"
        aria-hidden
      >
        <Trails placed={placed} links={links} flights={flights} highlight={highlight} />
      </svg>
      <PresenceToast peers={peers} />

      <div
        className="pointer-events-none absolute left-0 top-0 origin-top-left"
        style={{ width: VIEW_W, height: VIEW_H, transform: `translate(${fit.ox}px, ${fit.oy}px) scale(${fit.s})` }}
      >
        {/* Center: local device on the plaza. */}
        <div className="absolute text-center" style={anchorAt(CX, CY, SELF_SIZE)}>
          <div className="relative inline-block">
            <span className="anim-sonar-wave pointer-events-none absolute -inset-0.5 rounded-full border-2 border-sonar" />
            <div className="relative overflow-hidden rounded-full border-[3px] border-panel-2">
              {self ? (
                <Avatar
                  name={self.name}
                  fingerprint={self.fingerprint}
                  size={SELF_SIZE}
                  avatar={self.avatar}
                  src={srcOf(self.avatar)}
                />
              ) : (
                <div className="rounded-full bg-sonar-dim" style={{ width: SELF_SIZE, height: SELF_SIZE }} />
              )}
            </div>
          </div>
          <div className="mx-auto mt-2 w-fit max-w-44 truncate rounded-full border-2 border-line bg-panel px-3 py-0.5 text-xs font-bold text-fog shadow-[0_2px_0_rgba(41,71,51,0.12)]">
            {self ? `${self.name} · ${t.radar.myDevice}` : "…"}
          </div>
          <div className="isle-self-caption mt-1 text-[11px] tracking-[0.18em]">{t.radar.thisDevice}</div>
        </div>

        {/* Peers around the island. */}
        {placed.map(({ peer, leaving, pos, size }, i) => {
          const hovered = dragHover === peer.fingerprint;
          return (
            <button
              key={peer.fingerprint}
              data-peer={peer.fingerprint}
              onClick={() => onPeerClick(peer)}
              onMouseEnter={() => setHoverFp(peer.fingerprint)}
              onMouseLeave={() => setHoverFp((cur) => (cur === peer.fingerprint ? null : cur))}
              disabled={leaving}
              className="pointer-events-auto absolute cursor-pointer text-center"
              style={anchorAt(pos[0], pos[1], size)}
              title={`${peer.name} · ${peer.addrs[0] ?? ""}:${peer.port}`}
            >
              <div className={leaving ? "anim-pop-out" : "anim-pop-in"}>
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
                      size={size}
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
      </div>

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
