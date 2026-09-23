// SVG shapes for island props, drawn around their own origin (ground point).
// Colors come from --isle-* theme variables; animated parts use isle-* classes.

import type { CSSProperties } from "react";
import { mulberry32 } from "./geometry";
import type { PropKind, SceneProp } from "./scene";

/** Kinds that sway in the wind and cast a round shadow. */
export const TREE_KINDS: ReadonlySet<PropKind> = new Set(["pine", "round", "fruit", "bush", "palm"]);

/** Per-element animation timing through CSS custom properties. */
export const timing = (dur: number, delay = 0): CSSProperties =>
  ({ "--d": `${dur.toFixed(1)}s`, "--delay": `-${delay.toFixed(1)}s` }) as CSSProperties;

/** Rising chimney or campfire smoke. */
function Smoke({ x, y }: { x: number; y: number }) {
  return (
    <>
      {[0, 1.1, 2.2].map((d) => (
        <circle key={d} className="isle-smoke" style={timing(3.2, d)} cx={x} cy={y} r="3" fill="var(--isle-smoke)" />
      ))}
    </>
  );
}

/** Five-petal flower. */
function Flower({ x, y, tone }: { x: number; y: number; tone: number }) {
  return (
    <g transform={`translate(${x.toFixed(1)} ${y.toFixed(1)})`}>
      {[0, 72, 144, 216, 288].map((a) => (
        <circle
          key={a}
          cx={(2 * Math.sin((a * Math.PI) / 180)).toFixed(1)}
          cy={(-2 * Math.cos((a * Math.PI) / 180)).toFixed(1)}
          r="1.7"
          fill={`var(--isle-fl-${tone})`}
        />
      ))}
      <circle r="1.2" fill="var(--isle-fl-center)" />
    </g>
  );
}

/** Round deciduous canopy shared by plain and fruit trees. */
function Canopy({ fruit }: { fruit: boolean }) {
  return (
    <>
      <rect x="-1.6" y="0" width="3.2" height="8" rx="1.5" fill="var(--isle-trunk)" />
      <circle cx="0" cy="-5" r="8" fill="var(--isle-tree)" />
      <circle cx="-5" cy="-1" r="5.2" fill="var(--isle-tree-2)" />
      <circle cx="5" cy="-1" r="5.2" fill="var(--isle-tree)" />
      {fruit ? (
        [
          [-3, -7],
          [3, -4],
          [-5, 0],
          [4, 1],
        ].map(([x, y]) => <circle key={`${x}${y}`} cx={x} cy={y} r="1.7" fill="var(--isle-fruit)" />)
      ) : (
        <circle cx="-2.5" cy="-8" r="2.6" fill="var(--isle-tree-hi)" />
      )}
    </>
  );
}

/** Draws one prop kind; `mailFlag` raises the mailbox flag. */
export function PropShape({ prop, mailFlag }: { prop: Pick<SceneProp, "kind" | "roof" | "tone" | "seed">; mailFlag?: boolean }) {
  switch (prop.kind) {
    case "pine":
      return (
        <>
          <path d="M0 -22 L9 -4 L-9 -4Z" fill="var(--isle-pine)" />
          <path d="M0 -16 L11 2 L-11 2Z" fill="var(--isle-pine-2)" />
          <rect x="-1.6" y="2" width="3.2" height="6" fill="var(--isle-trunk)" />
        </>
      );
    case "round":
      return <Canopy fruit={false} />;
    case "fruit":
      return <Canopy fruit />;
    case "bush":
      return (
        <>
          <circle cx="-5" cy="2" r="5.5" fill="var(--isle-bush)" />
          <circle cx="4" cy="1" r="6.5" fill="var(--isle-bush)" />
          <circle cx="1" cy="-3" r="5" fill="var(--isle-tree-hi)" />
        </>
      );
    case "palm":
      return (
        <>
          <path d="M0 8 Q2 -4 0 -14" stroke="var(--isle-trunk)" strokeWidth="2.6" fill="none" />
          {[-60, -20, 20, 60, 180].map((a) => (
            <path
              key={a}
              d="M0 -14 q8 -6 14 2"
              stroke="var(--isle-tree)"
              strokeWidth="3"
              fill="none"
              strokeLinecap="round"
              transform={`rotate(${a} 0 -14)`}
            />
          ))}
          <circle cx="-1.5" cy="-13" r="1.6" fill="var(--isle-trunk)" />
          <circle cx="1.6" cy="-12.4" r="1.6" fill="var(--isle-trunk)" />
        </>
      );
    case "house":
      return (
        <>
          <rect x="4" y="-20" width="4" height="8" fill="var(--isle-chimney)" />
          <Smoke x={6} y={-22} />
          <rect x="-10" y="-5" width="20" height="14" rx="1.5" fill="var(--isle-wall)" />
          <path d="M-13 -4 L0 -16 L13 -4Z" fill={prop.roof === "b" ? "var(--isle-roof-b)" : "var(--isle-roof)"} />
          <rect x="-6.5" y="-1" width="5" height="5" rx="1" fill="var(--isle-lamp)" opacity=".9" />
          <rect x="2" y="1" width="5" height="8" rx="1.2" fill="var(--isle-wood-2)" />
        </>
      );
    case "garden":
      return (
        <>
          <rect x="-20" y="-9" width="40" height="18" rx="3" fill="var(--isle-soil)" />
          {[-4, 2].flatMap((y) =>
            [-15, -9, -3, 3, 9, 15].map((x) => (
              <path
                key={`${x},${y}`}
                d={`M${x - 2} ${y} l2 -3 l2 3`}
                stroke="var(--isle-sprout)"
                strokeWidth="1.6"
                fill="none"
                strokeLinecap="round"
              />
            )),
          )}
          {[-22, -11, 0, 11, 22].map((x) => (
            <rect key={x} x={x - 0.8} y="4" width="1.6" height="8" fill="var(--isle-wood-2)" />
          ))}
          <rect x="-22" y="6" width="44" height="1.4" fill="var(--isle-wood)" />
          <rect x="-22" y="9.5" width="44" height="1.4" fill="var(--isle-wood)" />
        </>
      );
    case "tent":
      return (
        <>
          <path d="M-12 8 L0 -12 L12 8Z" fill="var(--isle-roof)" />
          <path d="M0 -12 L12 8 L4 8Z" fill="var(--isle-roof-b)" opacity=".5" />
          <path d="M-3 8 L0 0 L3 8Z" fill="var(--isle-wood-2)" />
          <path d="M0 -12 l0 -4" stroke="var(--isle-wood-2)" strokeWidth="1.4" />
        </>
      );
    case "campfire":
      return (
        <>
          <circle cy="-2" r="12" fill="var(--isle-glow)" className="isle-flicker" />
          <path d="M-6 5 L6 1 M-6 1 L6 5" stroke="var(--isle-wood-2)" strokeWidth="2.4" strokeLinecap="round" />
          <path className="isle-flicker" style={timing(0.8)} d="M0 -8 Q5 -2 0 2 Q-5 -2 0 -8Z" fill="var(--isle-flame)" />
          <path d="M0 -4 Q2.4 -1 0 1.4 Q-2.4 -1 0 -4Z" fill="var(--isle-flame-core)" />
          <Smoke x={1} y={-12} />
        </>
      );
    case "lamp":
      return (
        <>
          <circle cy="-15" r="10" fill="var(--isle-glow)" className="isle-flicker" />
          <rect x="-.9" y="-13" width="1.8" height="15" fill="var(--isle-wood-2)" />
          <rect x="-3" y="-18" width="6" height="6" rx="1.4" fill="var(--isle-lamp)" />
        </>
      );
    case "mailbox":
      return (
        <>
          <rect x="-.9" y="-6" width="1.8" height="12" fill="var(--isle-wood-2)" />
          <rect x="-6" y="-13" width="12" height="8" rx="3.5" fill="var(--isle-mail)" />
          {/* The flag lies flat and springs up while files are arriving. */}
          <g className={`isle-mail-flag${mailFlag ? " up" : ""}`}>
            <rect x="5" y="-16" width="1.4" height="7" fill="var(--isle-wood-2)" />
            <rect x="5" y="-16" width="5" height="3" fill="var(--isle-flag)" />
          </g>
        </>
      );
    case "umbrella":
      return (
        <>
          <rect x="-14" y="2" width="20" height="9" rx="1.5" fill="var(--isle-roof-b)" opacity=".85" />
          <rect x="-.8" y="-14" width="1.6" height="18" fill="var(--isle-wood-2)" />
          <path d="M-14 -12 Q0 -24 14 -12Z" fill="var(--isle-umbrella)" />
          <path d="M-5 -12 Q0 -24 5 -12Z" fill="var(--isle-wall)" />
        </>
      );
    case "sandcastle":
      return (
        <>
          <rect x="-9" y="-6" width="18" height="10" fill="var(--isle-sand-edge)" />
          <rect x="-11" y="-12" width="6" height="10" fill="var(--isle-sand-edge)" />
          <rect x="5" y="-12" width="6" height="10" fill="var(--isle-sand-edge)" />
          <rect x="-3" y="-16" width="6" height="10" fill="var(--isle-sand-edge)" />
          <rect x="-.4" y="-24" width=".8" height="8" fill="var(--isle-wood-2)" />
          <path d="M.4 -24 l5 2 l-5 2Z" fill="var(--isle-stripe)" />
        </>
      );
    case "starfish":
      return (
        <path
          d="M0 -5 L1.4 -1.6 L5 -1.4 L2.2 1 L3.2 4.6 L0 2.6 L-3.2 4.6 L-2.2 1 L-5 -1.4 L-1.4 -1.6Z"
          fill="var(--isle-starfish)"
        />
      );
    case "shell":
      return <path d="M-4 2 Q0 -6 4 2Z" fill="var(--isle-wall)" stroke="var(--isle-sand-edge)" strokeWidth=".8" />;
    case "flowers": {
      const r = mulberry32(prop.seed ?? 1);
      const tone = prop.tone ?? 1;
      return (
        <>
          <ellipse cx="0" cy="1" rx="17" ry="9" fill="var(--isle-tree-hi)" opacity=".35" />
          {Array.from({ length: 8 }, (_, i) => (
            <Flower key={i} x={(r() - 0.5) * 26} y={(r() - 0.5) * 12} tone={((tone + (i % 2)) % 5) + 1} />
          ))}
        </>
      );
    }
    case "mushrooms":
      return (
        <>
          {[
            [-4, 2, 1],
            [3, 0, 0.8],
            [0, 5, 0.7],
          ].map(([x, y, k]) => (
            <g key={`${x}${y}`} transform={`translate(${x} ${y}) scale(${k})`}>
              <rect x="-1.4" y="-3" width="2.8" height="5" rx="1" fill="var(--isle-stem)" />
              <path d="M-5 -3 Q0 -10 5 -3Z" fill="var(--isle-mush)" />
              <circle cx="-1.6" cy="-5.4" r=".9" fill="var(--isle-stem)" />
              <circle cx="1.8" cy="-4.6" r=".8" fill="var(--isle-stem)" />
            </g>
          ))}
        </>
      );
    case "rock":
      return (
        <>
          <ellipse cx="0" cy="2" rx="9" ry="5.5" fill="var(--isle-rock)" />
          <ellipse cx="-2" cy="0" rx="4.5" ry="2.2" fill="var(--isle-rock-hi)" />
          <ellipse cx="7" cy="4" rx="4" ry="2.6" fill="var(--isle-rock)" />
        </>
      );
  }
}
