// Live map layer: trails to peers, transfer dots and arrival puffs.

import { memo, type CSSProperties } from "react";
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

/** Trail, transfer dots and ground effects for one peer. */
function PeerTrail({ p, link, highlight }: { p: PlacedPeer; link?: "send" | "recv"; highlight: string | null }) {
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
  highlight,
}: {
  placed: PlacedPeer[];
  links: TransferLinks;
  highlight: string | null;
}) {
  return (
    <>
      {placed.map((p) => (
        <PeerTrail key={p.peer.fingerprint} p={p} link={links.get(p.peer.fingerprint)} highlight={highlight} />
      ))}
    </>
  );
});
