// Live map layer: trails to peers, transfer dots, note flights and arrival puffs.

import { memo, useCallback, type CSSProperties } from "react";
import { NOTE_FLIGHT_MS, type NoteFlight } from "../../types";
import { f1 } from "./geometry";
import { trailPath, type PlacedPeer } from "./layout";

/** Direction of a running transfer, keyed by peer fingerprint. */
export type TransferLinks = ReadonlyMap<string, "send" | "recv">;

/** Dust puff offsets around an arriving peer's feet. */
const PUFFS = [
  [-18, 2],
  [18, 2],
  [-10, 8],
  [11, 9],
] as const;

/** A paper note that flies once along a trail after a text message. */
function FlyingNote({ d, direction }: { d: string; direction: NoteFlight["direction"] }) {
  // Animations inserted after load resolve begin="0s" against the document
  // timeline, which has long passed, so they are started explicitly on mount.
  const start = useCallback((g: SVGGElement | null) => {
    g?.querySelectorAll<SVGAnimationElement>("animate, animateMotion").forEach((a) => a.beginElement());
  }, []);
  const dur = `${NOTE_FLIGHT_MS}ms`;
  return (
    <g ref={start} opacity="0">
      <animateMotion
        dur={dur}
        begin="indefinite"
        fill="freeze"
        path={d}
        keyPoints={direction === "send" ? "0;1" : "1;0"}
        keyTimes="0;1"
        calcMode="spline"
        keySplines="0.4 0 0.2 1"
      />
      <animate attributeName="opacity" dur={dur} begin="indefinite" fill="freeze" values="0;1;1;0" keyTimes="0;0.15;0.8;1" />
      <g transform="rotate(-12)">
        <rect x="-11" y="-8" width="22" height="16" rx="3" fill="var(--isle-note)" stroke="var(--isle-note-edge)" strokeWidth="1.2" />
        <path d="M-6 -2.5 H5 M-6 2.5 H2" stroke="var(--color-sonar)" strokeWidth="2.2" strokeLinecap="round" />
      </g>
    </g>
  );
}

/** Trail, transfer dots, note flights and ground effects for one peer. */
function PeerTrail({
  p,
  link,
  flights,
  highlight,
}: {
  p: PlacedPeer;
  link?: "send" | "recv";
  flights: NoteFlight[];
  highlight: string | null;
}) {
  const fp = p.peer.fingerprint;
  const d = trailPath(p);
  const [x, y] = p.pos;
  const state = highlight === null ? "" : highlight === fp ? " hot" : " dim";
  return (
    <g>
      {/* The mask draws the dotted trail outward on arrival and back in on departure. */}
      <mask id={`isle-trail-${fp}`} maskUnits="userSpaceOnUse">
        <path
          d={d}
          pathLength={1}
          fill="none"
          stroke="#fff"
          strokeWidth="16"
          className={p.leaving ? "isle-retract" : "isle-reveal"}
        />
      </mask>
      <g className={`isle-trail${state}`} mask={`url(#isle-trail-${fp})`}>
        <path d={d} fill="none" stroke="var(--isle-path-under)" strokeWidth="7" strokeLinecap="round" />
        <path
          className="isle-trail-dots"
          d={d}
          fill="none"
          stroke="var(--isle-path)"
          strokeWidth="4"
          strokeLinecap="round"
          strokeDasharray="0.1 9"
        />
      </g>
      {link && !p.leaving &&
        [0, 0.45, 0.9].map((b) => (
          <circle key={b} r="3.6" fill="var(--color-ember)" stroke="var(--isle-dot-ring)" strokeWidth="1.2">
            {/* Sending flows outward along the trail, receiving flows back in. */}
            <animateMotion
              dur="1.35s"
              begin={`-${b}s`}
              repeatCount="indefinite"
              path={d}
              keyPoints={link === "send" ? "0;1" : "1;0"}
              keyTimes="0;1"
              calcMode="linear"
            />
          </circle>
        ))}
      {!p.leaving && flights.map((f) => <FlyingNote key={f.id} d={d} direction={f.direction} />)}
      {!p.leaving && (
        <>
          <ellipse cx={f1(x)} cy={f1(y + p.size * 0.46)} rx={p.size * 0.44} ry={p.size * 0.13} fill="var(--isle-shadow)" />
          {/* One-shot landing effects: they play once when the element mounts. */}
          <circle className="isle-burst" cx={f1(x)} cy={f1(y)} r={p.size / 2} fill="none" stroke="var(--color-sonar)" strokeWidth="2" />
          {PUFFS.map(([dx, dy]) => (
            <circle
              key={dx}
              className="isle-puff"
              style={{ "--dx": `${dx}px`, "--dy": `${dy}px` } as CSSProperties}
              cx={f1(x)}
              cy={f1(y + p.size * 0.45)}
              r="5"
              fill="var(--isle-sand)"
            />
          ))}
        </>
      )}
    </g>
  );
}

/** Dynamic layer above the scenery. */
export const Trails = memo(function Trails({
  placed,
  links,
  flights,
  highlight,
}: {
  placed: PlacedPeer[];
  links: TransferLinks;
  flights: NoteFlight[];
  highlight: string | null;
}) {
  return (
    <>
      {placed.map((p) => (
        <PeerTrail
          key={p.peer.fingerprint}
          p={p}
          link={links.get(p.peer.fingerprint)}
          flights={flights.filter((f) => f.fingerprint === p.peer.fingerprint)}
          highlight={highlight}
        />
      ))}
    </>
  );
});
