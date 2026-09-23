// Island map geometry: design space, seeded noise, and shape helpers.
//
// Everything is laid out in a fixed 800×752 design space that the map scales
// uniformly ("meet") into the available area. Angles are bearings: 0° points
// up and grows clockwise. The island is viewed slightly from above, so vertical
// distances are squashed by SQ.

/** Design-space width and height. */
export const VIEW_W = 800;
export const VIEW_H = 752;
/** Island center, where the local device stands. */
export const CX = 400;
export const CY = 368;
/** Vertical squash of the top-down view. */
export const SQ = 0.8;

export type Pt = readonly [number, number];

/** Deterministic PRNG so the scenery is identical on every launch. */
export function mulberry32(seed: number): () => number {
  let a = seed;
  return () => {
    a |= 0;
    a = (a + 0x6d2b79f5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

/** Rounds a coordinate for compact SVG path data. */
export const f1 = (n: number): string => n.toFixed(1);

/** Euclidean distance between two points. */
export const dist = (a: Pt, b: Pt): number => Math.hypot(a[0] - b[0], a[1] - b[1]);

/** Point at radius r and bearing deg around the island center. */
export function at(r: number, deg: number): Pt {
  const t = (deg * Math.PI) / 180;
  return [CX + r * Math.sin(t), CY - r * SQ * Math.cos(t)];
}

/** Closed Catmull-Rom spline through the points as cubic Bézier path data. */
export function smoothClosed(pts: Pt[]): string {
  const n = pts.length;
  let d = `M${f1(pts[0][0])} ${f1(pts[0][1])}`;
  for (let i = 0; i < n; i++) {
    const p0 = pts[(i - 1 + n) % n];
    const p1 = pts[i];
    const p2 = pts[(i + 1) % n];
    const p3 = pts[(i + 2) % n];
    d += ` C${f1(p1[0] + (p2[0] - p0[0]) / 6)} ${f1(p1[1] + (p2[1] - p0[1]) / 6)} ${f1(p2[0] - (p3[0] - p1[0]) / 6)} ${f1(p2[1] - (p3[1] - p1[1]) / 6)} ${f1(p2[0])} ${f1(p2[1])}`;
  }
  return `${d}Z`;
}

/** Organic closed outline around (cx, cy) with seeded low-frequency wobble. */
export function blob(cx: number, cy: number, rx: number, ry: number, seed: number, amp: number, n = 72): string {
  const r = mulberry32(seed);
  const ph = [r() * 6.28, r() * 6.28, r() * 6.28, r() * 6.28];
  const pts: Pt[] = [];
  for (let i = 0; i < n; i++) {
    const t = (i / n) * Math.PI * 2;
    const k =
      1 +
      amp *
        (0.45 * Math.sin(2 * t + ph[0]) +
          0.3 * Math.sin(3 * t + ph[1]) +
          0.18 * Math.sin(5 * t + ph[2]) +
          0.1 * Math.sin(8 * t + ph[3]));
    pts.push([cx + rx * k * Math.sin(t), cy - ry * k * Math.cos(t)]);
  }
  return smoothClosed(pts);
}

/** Samples a quadratic Bézier curve. */
export function quadPoints(a: Pt, c: Pt, b: Pt, n = 36): Pt[] {
  const out: Pt[] = [];
  for (let i = 0; i <= n; i++) {
    const t = i / n;
    const u = 1 - t;
    out.push([u * u * a[0] + 2 * u * t * c[0] + t * t * b[0], u * u * a[1] + 2 * u * t * c[1] + t * t * b[1]]);
  }
  return out;
}

/** Radius of the main island's grass and of the plaza under the local device. */
export const ISLAND_R = 362;
export const PLAZA_R = 94;
