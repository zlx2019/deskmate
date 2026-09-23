// Static island scenery: sea, islets, grass, props and ambient life.
// Rendered once; only prop clearing and the mailbox flag change afterwards.

import { memo } from "react";
import { f1 } from "./geometry";
import { PropShape, TREE_KINDS, timing } from "./props";
import { SCENE, type SceneProp } from "./scene";

/** Gull wing poses for the flapping loop. */
const WING_UP = "M-6 0 Q-3 -4 0 0 Q3 -4 6 0";
const WING_DOWN = "M-6 -2 Q-3 1 0 0 Q3 1 6 -2";

/** One clearable prop; memoized so only props whose state flips re-render. */
const PropItem = memo(function PropItem({
  prop,
  cleared,
  mailFlag,
}: {
  prop: SceneProp;
  cleared: boolean;
  mailFlag: boolean;
}) {
  const tree = TREE_KINDS.has(prop.kind);
  const sway = tree && prop.id % 3 === 0;
  const shadow = tree || prop.kind === "house" || prop.kind === "tent";
  return (
    <g transform={`translate(${f1(prop.x)} ${f1(prop.y)}) scale(${prop.k.toFixed(2)})`}>
      <g className={`isle-prop${cleared ? " cleared" : ""}`}>
        {shadow && <ellipse cx="2" cy="9" rx={prop.kind === "house" ? 13 : 9} ry="3" fill="var(--isle-shadow)" />}
        <g className={sway ? "isle-sway" : undefined} style={sway ? timing(4 + (prop.id % 5) * 0.6, (prop.id % 7) * 0.7) : undefined}>
          <PropShape prop={prop} mailFlag={mailFlag} />
        </g>
      </g>
    </g>
  );
});

/** Sea texture: waves, sun sparkles (day) and star reflections (night). */
function Sea() {
  return (
    <>
      {SCENE.waves.map((w, i) => (
        <path
          key={i}
          className="isle-wave"
          style={timing(w.dur, w.delay)}
          d={`M${f1(w.x)} ${f1(w.y)} q5 -4 10 0 q5 -4 10 0`}
          fill="none"
          stroke="var(--isle-wave)"
          strokeWidth="1.6"
          strokeLinecap="round"
        />
      ))}
      {SCENE.sparkles.map((s, i) => (
        <path
          key={i}
          className="isle-sparkle"
          style={timing(s.dur, s.delay)}
          d={`M${f1(s.x)} ${f1(s.y - 4)} L${f1(s.x + 1)} ${f1(s.y - 1)} L${f1(s.x + 4)} ${f1(s.y)} L${f1(s.x + 1)} ${f1(s.y + 1)} L${f1(s.x)} ${f1(s.y + 4)} L${f1(s.x - 1)} ${f1(s.y + 1)} L${f1(s.x - 4)} ${f1(s.y)} L${f1(s.x - 1)} ${f1(s.y - 1)}Z`}
          fill="var(--isle-sparkle)"
        />
      ))}
      {SCENE.stars.map((s, i) => (
        <circle key={i} className="isle-sparkle" style={timing(s.dur, s.delay)} cx={f1(s.x)} cy={f1(s.y)} r="1.2" fill="var(--isle-star)" />
      ))}
    </>
  );
}

/** Main island ground: shallows, beach, foam line, grass and plaza. */
function Ground() {
  const { pond } = SCENE;
  return (
    <>
      {SCENE.islets.map((isl, i) => (
        <g key={i}>
          <path d={isl.shallow} fill="var(--isle-shallow)" />
          <path d={isl.sand} fill="var(--isle-sand)" stroke="var(--isle-sand-edge)" strokeWidth="2" />
          <path d={isl.land} fill="var(--isle-land)" stroke="var(--isle-contour)" strokeWidth="1.4" />
          <path className="isle-foam" d={isl.foam} fill="none" stroke="var(--isle-foam)" strokeWidth="1.6" />
        </g>
      ))}
      <path d={SCENE.shallow} fill="var(--isle-shallow)" />
      <path d={SCENE.beach} fill="var(--isle-sand)" stroke="var(--isle-sand-edge)" strokeWidth="2" />
      <path className="isle-foam" d={SCENE.foam} fill="none" stroke="var(--isle-foam)" strokeWidth="2" />
      {/* Only the grass edge is outlined; the plaza is a soft clearing. */}
      <path d={SCENE.land} fill="var(--isle-land)" stroke="var(--isle-contour)" strokeWidth="1.6" />
      <path d={SCENE.land} fill="none" stroke="var(--isle-contour-hi)" strokeWidth="1.2" transform="translate(0 1.8)" />
      <path d={SCENE.plaza} fill="var(--isle-plaza)" />
      {/* Pond with ripples and lily pads. */}
      <path d={SCENE.pondPath} fill="var(--isle-shallow)" stroke="var(--isle-sand)" strokeWidth="3" />
      {[
        [-10, -2, 0],
        [12, 4, 1.8],
      ].map(([dx, dy, delay]) => (
        <ellipse
          key={dx}
          className="isle-ripple"
          style={timing(3.6, delay)}
          cx={f1(pond[0] + dx)}
          cy={f1(pond[1] + dy)}
          rx="9"
          ry="3.5"
          fill="none"
          stroke="var(--isle-foam)"
          strokeWidth="1.2"
        />
      ))}
      <path d={`M${f1(pond[0] + 14)} ${f1(pond[1] - 6)} a5 3 0 1 0 5 1 l-5 -1Z`} fill="var(--isle-tree-hi)" />
      <path d={`M${f1(pond[0] - 20)} ${f1(pond[1] + 5)} a4 2.5 0 1 0 4 1 l-4 -1Z`} fill="var(--isle-tree-hi)" />
      {SCENE.beachRocks.map((r, i) => (
        <ellipse key={i} cx={f1(r.x)} cy={f1(r.y)} rx={f1(r.rx)} ry={f1(r.ry)} fill="var(--isle-rock)" />
      ))}
      {SCENE.tufts.map(([x, y], i) => (
        <path
          key={i}
          d={`M${f1(x - 3)} ${f1(y)} l1 -4 M${f1(x)} ${f1(y)} l0 -5 M${f1(x + 3)} ${f1(y)} l-1 -4`}
          stroke="var(--isle-tuft)"
          strokeWidth="1.2"
          strokeLinecap="round"
        />
      ))}
      {/* Stream from the pond to the sea with a small bridge. */}
      <path d={SCENE.river} fill="none" stroke="var(--isle-sand)" strokeWidth="11" strokeLinecap="round" />
      <path d={SCENE.river} fill="none" stroke="var(--isle-shallow)" strokeWidth="7" strokeLinecap="round" />
      <path className="isle-foam isle-stream" d={SCENE.river} fill="none" stroke="var(--isle-foam)" strokeWidth="1.2" />
      <g transform={`translate(${f1(pond[0] + 52)} ${f1(pond[1] + 24)}) rotate(-28)`}>
        <rect x="-4" y="-11" width="8" height="22" fill="var(--isle-wood)" />
        {[-8, -3, 2, 7].map((y) => (
          <rect key={y} x="-5" y={y} width="10" height="1.2" fill="var(--isle-wood-2)" />
        ))}
        <rect x="-6" y="-12" width="1.6" height="24" fill="var(--isle-wood-2)" />
        <rect x="4.4" y="-12" width="1.6" height="24" fill="var(--isle-wood-2)" />
      </g>
      {/* Ducks paddling in the pond. */}
      {[
        [-12, -4, 7],
        [10, 3, 9],
      ].map(([dx, dy, dur]) => (
        <g key={dx} transform={`translate(${f1(pond[0] + dx)} ${f1(pond[1] + dy)})`}>
          <g className="isle-paddle" style={timing(dur)}>
            <g className="isle-duck">
              <ellipse cx="0" cy="0" rx="4.6" ry="3" fill="var(--isle-duck)" />
              <circle cx="3.4" cy="-2.6" r="2.2" fill="var(--isle-duck)" />
              <path d="M5.4 -2.6 l2.4 .6 l-2.4 .6Z" fill="var(--isle-beak)" />
            </g>
          </g>
        </g>
      ))}
    </>
  );
}

/** Corner islet landmarks: windmill, lighthouse, cottage with dock, sailboat. */
function Landmarks() {
  return (
    <>
      {SCENE.isletTrees.map((t, i) => (
        <g key={i} transform={`translate(${t.x} ${t.y}) scale(0.9)`}>
          <ellipse cx="2" cy="9" rx="9" ry="3" fill="var(--isle-shadow)" />
          <PropShape prop={t} />
        </g>
      ))}
      <g transform="translate(62 58)">
        <path d="M-10 18 L-7 -8 L7 -8 L10 18Z" fill="var(--isle-wall)" />
        <path d="M-9 -8 L0 -18 L9 -8Z" fill="var(--isle-roof)" />
        <rect x="-3" y="8" width="6" height="10" rx="3" fill="var(--isle-wood-2)" />
        <circle cx="0" cy="-1" r="2.4" fill="var(--isle-lamp)" opacity=".9" />
        <g transform="translate(0 -9)">
          <g className="isle-spin">
            {[0, 90, 180, 270].map((a) => (
              <g key={a} transform={`rotate(${a})`}>
                <rect x="-1.2" y="-25" width="2.4" height="25" fill="var(--isle-wood-2)" />
                <rect x="1.2" y="-24" width="6" height="17" fill="var(--isle-wall)" stroke="var(--isle-wood-2)" strokeWidth=".8" />
              </g>
            ))}
            <circle r="2.6" fill="var(--isle-wood-2)" />
          </g>
        </g>
      </g>
      <g transform="translate(726 680)">
        {/* The beam turns around the lamp: its tip is the fill-box origin (100% 50%). */}
        <path className="isle-beam" d="M0 -38 L-120 -68 L-120 -8Z" fill="url(#isle-beam-grad)" />
        <circle cy="-38" r="13" fill="var(--isle-glow)" className="isle-flicker" />
        <rect x="-40" y="10" width="34" height="5" fill="var(--isle-wood)" />
        <rect x="-38" y="15" width="2.5" height="7" fill="var(--isle-wood-2)" />
        <rect x="-14" y="15" width="2.5" height="7" fill="var(--isle-wood-2)" />
        <path d="M-9 14 L-6 -30 L6 -30 L9 14Z" fill="var(--isle-wall)" />
        <path d="M-7.6 -6 L7.6 -6 L8.1 2 L-8.1 2Z" fill="var(--isle-stripe)" />
        <path d="M-6.4 -24 L6.4 -24 L6.8 -16 L-6.8 -16Z" fill="var(--isle-stripe)" />
        <rect x="-9" y="-33" width="18" height="3" rx="1" fill="var(--isle-wood-2)" />
        <rect x="-5" y="-42" width="10" height="9" rx="2" fill="var(--isle-lamp)" />
        <path d="M-7 -42 L0 -50 L7 -42Z" fill="var(--isle-stripe)" />
      </g>
      <g transform="translate(58 690)">
        <ellipse cx="1" cy="9" rx="11" ry="3" fill="var(--isle-shadow)" />
        <rect x="-8" y="-4" width="16" height="12" rx="1.5" fill="var(--isle-wall)" />
        <path d="M-10.5 -3 L0 -13 L10.5 -3Z" fill="var(--isle-roof)" />
        <rect x="-5" y="0" width="4" height="4" rx=".8" fill="var(--isle-lamp)" />
        <rect x="2" y="1" width="4" height="7" rx="1" fill="var(--isle-wood-2)" />
      </g>
      <rect x="96" y="700" width="30" height="5" fill="var(--isle-wood)" />
      <rect x="120" y="705" width="2.5" height="7" fill="var(--isle-wood-2)" />
      <g transform="translate(160 722)">
        <g className="isle-boat-drift">
          <g className="isle-boat">
            <path d="M-18 0 L18 0 L11 9 L-11 9Z" fill="var(--isle-wood)" />
            <path d="M-1 -2 L-1 -30 L16 -4Z" fill="var(--isle-wall)" />
            <path d="M-4 -4 L-4 -22 L-15 -4Z" fill="var(--isle-flag)" />
          </g>
        </g>
      </g>
    </>
  );
}

/** Butterflies, fireflies, drifting clouds and gulls above the island. */
function Sky() {
  return (
    <>
      {SCENE.butterflies.map(([x, y], i) => (
        <g key={i}>
          <g transform="translate(0 -14)">
            <g className="isle-flap" style={{ animationDuration: `${0.24 + i * 0.03}s` }}>
              <path
                d="M0 0 q-5 -5 -6 0 q1 4 6 0 q5 -5 6 0 q-1 4 -6 0Z"
                fill={i % 2 ? "var(--isle-butterfly-2)" : "var(--isle-butterfly)"}
              />
            </g>
          </g>
          <animateMotion
            dur={`${6 + i}s`}
            repeatCount="indefinite"
            path={`M${f1(x)} ${f1(y)} c 14 -10 22 6 8 12 c -14 6 -26 -6 -8 -12Z`}
          />
        </g>
      ))}
      {SCENE.fireflies.map((f, i) => (
        <circle key={i} className="isle-firefly" style={timing(f.dur, f.delay)} cx={f1(f.x)} cy={f1(f.y)} r="1.8" fill="var(--isle-firefly)" />
      ))}
      {[
        [150, 70, 0],
        [470, 95, 30],
        [300, 50, 52],
      ].map(([y, dur, delay]) => (
        <g key={y} className="isle-drift" style={timing(dur, delay)}>
          <g fill="var(--isle-cloud-shadow)" transform="translate(24 50)">
            <ellipse cx="0" cy={y} rx="36" ry="11" />
            <ellipse cx="24" cy={y - 5} rx="21" ry="9" />
          </g>
          <g fill="var(--isle-cloud)">
            <ellipse cx="0" cy={y} rx="36" ry="12" />
            <ellipse cx="24" cy={y - 8} rx="22" ry="11" />
            <ellipse cx="-19" cy={y - 4} rx="16" ry="8" />
          </g>
        </g>
      ))}
      {[
        [0, 120, 24],
        [9, 180, 26],
        [14, 150, 30],
      ].map(([delay, y, dur], i) => (
        <g key={i} className="isle-drift" style={timing(dur, delay)}>
          <g transform={`translate(${i * 16} ${y + (i % 2) * 10})`}>
            <path d={WING_UP} fill="none" stroke="var(--isle-bird)" strokeWidth="1.8" strokeLinecap="round">
              <animate attributeName="d" values={`${WING_UP};${WING_DOWN};${WING_UP}`} dur={`${0.7 + i * 0.1}s`} repeatCount="indefinite" />
            </path>
          </g>
        </g>
      ))}
    </>
  );
}

/** Island scenery; `cleared` lists prop ids that step aside for peers. */
export const IslandBackdrop = memo(function IslandBackdrop({
  cleared,
  mailFlag,
}: {
  cleared: ReadonlySet<number>;
  mailFlag: boolean;
}) {
  return (
    <>
      <defs>
        <linearGradient id="isle-beam-grad" x1="1" y1="0" x2="0" y2="0">
          <stop offset="0" style={{ stopColor: "var(--isle-beam)" }} />
          <stop offset="1" style={{ stopColor: "var(--isle-beam)", stopOpacity: 0 }} />
        </linearGradient>
      </defs>
      <Sea />
      <Ground />
      {SCENE.props.map((p) => (
        <PropItem key={p.id} prop={p} cleared={cleared.has(p.id)} mailFlag={p.kind === "mailbox" && mailFlag} />
      ))}
      <Landmarks />
      <Sky />
    </>
  );
});
