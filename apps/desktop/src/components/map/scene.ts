// Static island scenery generated once from a fixed seed.
//
// The scene is plain data (positions and path strings); IslandBackdrop turns it
// into SVG. Props on the main island can "step aside" when a peer lands on
// them, so each carries a clearing radius. The generation order of random draws
// is part of the look: reordering calls reshuffles the whole island.

import { CX, CY, ISLAND_R, PLAZA_R, SQ, VIEW_H, VIEW_W, at, blob, dist, mulberry32, type Pt } from "./geometry";

export type PropKind =
  | "pine"
  | "round"
  | "fruit"
  | "bush"
  | "palm"
  | "house"
  | "garden"
  | "tent"
  | "campfire"
  | "lamp"
  | "mailbox"
  | "umbrella"
  | "sandcastle"
  | "starfish"
  | "shell"
  | "flowers"
  | "mushrooms"
  | "rock";

/** A clearable item on the main island. */
export interface SceneProp {
  id: number;
  kind: PropKind;
  x: number;
  y: number;
  /** Drawing scale. */
  k: number;
  /** Clearing radius used when peers or trails come close. */
  r: number;
  /** House roof color variant. */
  roof?: "a" | "b";
  /** Flower bed color offset (1–5). */
  tone?: number;
  /** Seed for per-item detail such as flower placement. */
  seed?: number;
}

/** A looping sprite with its own timing. */
export interface Twinkle {
  x: number;
  y: number;
  dur: number;
  delay: number;
}

/** Small decorative island in a corner. */
export interface Islet {
  shallow: string;
  sand: string;
  land: string;
  foam: string;
}

export interface Scene {
  waves: Twinkle[];
  sparkles: Twinkle[];
  stars: Twinkle[];
  islets: Islet[];
  /** Main island outlines: shallows, beach, foam line. */
  shallow: string;
  beach: string;
  foam: string;
  /** Grass of the main island and the plaza under the local device. */
  land: string;
  plaza: string;
  pond: Pt;
  pondPath: string;
  river: string;
  beachRocks: { x: number; y: number; rx: number; ry: number }[];
  tufts: Pt[];
  props: SceneProp[];
  /** Trees on the corner islets (never cleared). */
  isletTrees: { x: number; y: number; kind: PropKind }[];
  butterflies: Pt[];
  fireflies: Twinkle[];
}

/** Drawing scale per structure kind; trees get a random scale instead. */
const PROP_SCALE: Partial<Record<PropKind, number>> = {
  house: 1.35,
  garden: 1.25,
  tent: 1.35,
  campfire: 1.3,
  mailbox: 1.35,
  lamp: 1.25,
  flowers: 1.2,
  mushrooms: 1.35,
  rock: 1.25,
};

/** Waves in the margins outside the design area for non-matching aspect ratios. */
function edgeWaves(): Twinkle[] {
  const rnd = mulberry32(7);
  const out: Twinkle[] = [];
  for (let i = 0; i < 90; i++) {
    const x = -320 + rnd() * (VIEW_W + 640);
    const y = -240 + rnd() * (VIEW_H + 480);
    if (x > -10 && x < VIEW_W + 10 && y > -10 && y < VIEW_H + 10) continue;
    out.push({ x, y, dur: 5 + rnd() * 4, delay: rnd() * 8 });
  }
  return out;
}

/** Builds the island scenery. */
function buildScene(): Scene {
  const rnd = mulberry32(42);
  const waves: Twinkle[] = [];
  for (let i = 0; i < 80; i++) {
    const x = rnd() * VIEW_W;
    const y = rnd() * VIEW_H;
    if (((x - CX) / 415) ** 2 + ((y - CY) / 352) ** 2 < 1) continue;
    waves.push({ x, y, dur: 5 + rnd() * 4, delay: rnd() * 8 });
  }
  const sparkles: Twinkle[] = [];
  const stars: Twinkle[] = [];
  for (let i = 0; i < 18; i++) {
    const x = rnd() * VIEW_W;
    const y = rnd() * VIEW_H;
    if (((x - CX) / 420) ** 2 + ((y - CY) / 356) ** 2 < 1) continue;
    sparkles.push({ x, y, dur: 2 + rnd() * 3, delay: rnd() * 4 });
    const dur = 2 + rnd() * 3;
    const delay = rnd() * 4;
    stars.push({ x: rnd() * VIEW_W, y: rnd() * 150, dur, delay });
  }

  const islet = (x: number, y: number, rx: number, ry: number, seed: number): Islet => ({
    shallow: blob(x, y, rx + 16, ry + 14, seed, 0.12),
    sand: blob(x, y, rx, ry, seed, 0.12),
    land: blob(x, y - 3, rx - 12, ry - 12, seed + 1, 0.14),
    foam: blob(x, y, rx + 4, ry + 4, seed, 0.12),
  });
  const islets = [islet(60, 66, 62, 46, 90), islet(742, 692, 62, 46, 95), islet(748, 62, 50, 36, 97), islet(56, 698, 50, 36, 99)];

  const pond = at(300, 92);

  const beachRocks = [];
  for (let i = 0; i < 12; i++) {
    const t = ((rnd() * 360) * Math.PI) / 180;
    beachRocks.push({ x: CX + 373 * Math.sin(t), y: CY + 5 - 310 * Math.cos(t), rx: 4 + rnd() * 4, ry: 3 + rnd() * 2 });
  }

  // Props: structures first, then scattered details, then the forest fills the rest.
  const props: Omit<SceneProp, "id">[] = [];
  const occupied: { c: Pt; r: number }[] = [
    { c: [CX, CY], r: 96 },
    { c: pond, r: 50 },
  ];
  const free = (x: number, y: number, r: number) =>
    occupied.every((o) => dist([x, y], o.c) > o.r + r) && ((x - CX) / 346) ** 2 + ((y - CY) / (346 * SQ)) ** 2 < 1;
  const put = (x: number, y: number, kind: PropKind, r: number, extra: Partial<SceneProp> = {}) => {
    const k = PROP_SCALE[kind] ?? 1;
    props.push({ x, y, k, kind, r: r * k, ...extra });
    occupied.push({ c: [x, y], r: r * k });
  };
  // Village: two cottages and a fenced vegetable garden.
  const v = at(238, 160);
  put(v[0] - 30, v[1] - 8, "house", 16, { roof: "a" });
  put(v[0] + 18, v[1] - 22, "house", 16, { roof: "b" });
  put(v[0] - 4, v[1] + 26, "garden", 22);
  // Campsite: tent and campfire.
  const c = at(212, 322);
  put(c[0] - 16, c[1], "tent", 14);
  put(c[0] + 18, c[1] + 8, "campfire", 10);
  // Plaza lamps and the mailbox beside the local device.
  put(...at(86, 298), "lamp", 6);
  put(...at(86, 62), "lamp", 6);
  put(...at(98, 114), "mailbox", 8);
  // Beach items sit outside the forest ellipse and are placed directly.
  props.push(
    { x: 318, y: 672, kind: "umbrella", r: 16, k: 1.3 },
    { x: 512, y: 680, kind: "sandcastle", r: 12, k: 1.25 },
    { x: 150, y: 598, kind: "palm", r: 10, k: 0.9 },
    { x: 652, y: 600, kind: "palm", r: 10, k: 0.95 },
    { x: 226, y: 662, kind: "starfish", r: 6, k: 1.3 },
    { x: 604, y: 672, kind: "starfish", r: 6, k: 1.3 },
    { x: 262, y: 676, kind: "shell", r: 4, k: 1 },
    { x: 560, y: 684, kind: "shell", r: 4, k: 1 },
    { x: 420, y: 688, kind: "shell", r: 4, k: 1 },
  );
  const scatter = (count: number, kind: PropKind, r: number, rMin: number, rMax: number, gap: number) => {
    for (let placed = 0, tries = 0; placed < count && tries < 400; tries++) {
      const [x, y] = at(rMin + rnd() * (rMax - rMin), rnd() * 360);
      if (!free(x, y, r + gap)) continue;
      put(x, y, kind, r, { tone: 1 + Math.floor(rnd() * 5), seed: Math.floor(rnd() * 1e6) });
      placed++;
    }
  };
  scatter(7, "flowers", 16, 120, 320, 26);
  scatter(6, "mushrooms", 8, 130, 330, 12);
  scatter(6, "rock", 8, 120, 330, 14);
  const tree = (x: number, y: number, kind: PropKind) => {
    if (free(x, y, 6) && props.every((t) => dist([t.x, t.y], [x, y]) > 14)) {
      props.push({ x, y, k: 0.8 + rnd() * 0.4, kind, r: 8 });
    }
  };
  for (let a = 0; a < 360; a += 6 + rnd() * 7) {
    const [x, y] = at(330 + rnd() * 16, a);
    tree(x, y, rnd() < 0.55 ? "pine" : "round");
  }
  for (let cluster = 0; cluster < 30; cluster++) {
    const [kx, ky] = at(115 + rnd() * 220, rnd() * 360);
    // The bound is re-drawn on every check, matching the reviewed prototype.
    for (let k = 0; k < 3 + Math.floor(rnd() * 4); k++) {
      tree(
        kx + (rnd() - 0.5) * 62,
        ky + (rnd() - 0.5) * 44,
        rnd() < 0.2 ? "pine" : rnd() < 0.25 ? "bush" : rnd() < 0.3 ? "fruit" : "round",
      );
    }
  }
  const tufts: Pt[] = [];
  for (let i = 0; i < 36; i++) {
    const p = at(110 + rnd() * 230, rnd() * 360);
    if (free(p[0], p[1], 2)) tufts.push(p);
  }
  const river = `M${(pond[0] + 30).toFixed(1)} ${(pond[1] + 6).toFixed(1)} C ${(pond[0] + 50).toFixed(1)} ${(pond[1] + 14).toFixed(1)}, ${(pond[0] + 56).toFixed(1)} ${(pond[1] + 36).toFixed(1)}, ${(pond[0] + 92).toFixed(1)} ${(pond[1] + 52).toFixed(1)}`;

  const sorted = props.sort((a, b) => a.y - b.y).map((p, id) => ({ ...p, id }));
  const butterflies = sorted
    .filter((p) => p.kind === "flowers")
    .slice(0, 4)
    .map((p): Pt => [p.x, p.y]);
  const fireflies: Twinkle[] = [];
  for (let i = 0; i < 26; i++) {
    const [x, y] = at(115 + rnd() * 225, rnd() * 360);
    fireflies.push({ x, y, dur: 2.5 + rnd() * 3, delay: rnd() * 4 });
  }

  return {
    waves: [...waves, ...edgeWaves()],
    sparkles,
    stars,
    islets,
    shallow: blob(CX, CY + 8, 402, 338, 11, 0.07),
    beach: blob(CX, CY + 5, 382, 318, 11, 0.07),
    foam: blob(CX, CY + 5, 388, 324, 11, 0.07),
    land: blob(CX, CY, ISLAND_R, ISLAND_R * SQ, 21, 0.07),
    plaza: blob(CX, CY, PLAZA_R, PLAZA_R * SQ, 25, 0.1),
    pond,
    pondPath: blob(pond[0], pond[1], 36, 20, 77, 0.2),
    river,
    beachRocks,
    tufts,
    props: sorted,
    isletTrees: [
      { x: 26, y: 80, kind: "pine" },
      { x: 96, y: 84, kind: "round" },
      { x: 764, y: 704, kind: "round" },
      { x: 778, y: 684, kind: "pine" },
      { x: 736, y: 58, kind: "palm" },
      { x: 758, y: 66, kind: "palm" },
      { x: 34, y: 700, kind: "pine" },
      { x: 84, y: 706, kind: "round" },
    ],
    butterflies,
    fireflies,
  };
}

/** The island scenery, identical on every launch. */
export const SCENE: Scene = buildScene();
