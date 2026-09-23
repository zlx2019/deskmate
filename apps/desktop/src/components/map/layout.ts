// Peer placement on the island: each peer lands on a random free spot when it
// comes online and keeps it until it leaves.

import { useMemo, useRef } from "react";
import type { PeerDto } from "../../types";
import { CX, CY, at, dist, quadPoints, type Pt } from "./geometry";
import { SCENE, type SceneProp } from "./scene";

/** A peer positioned on the island together with its trail geometry. */
export interface PlacedPeer {
  peer: PeerDto;
  /** Whether the peer is playing its exit animation. */
  leaving: boolean;
  /** Ground point under the avatar. */
  pos: Pt;
  /** Control point of the trail from the island center. */
  ctrl: Pt;
  /** Avatar diameter in design units. */
  size: number;
}

/** Avatar diameter in design units. */
const AVATAR_SIZE = 48;
/** Landing ring: outside the plaza and its label, inside the forest rim. */
const R_MIN = 135;
const R_MAX = 325;
/** Label footprint kept free between two peers: the pill hangs below the
 * avatar, so stacked peers need room for avatar, pill and a small gap. */
const MIN_DX = 118;
const MIN_DY = 92;
/** Candidate spots tried per landing. */
const TRIES = 80;
/** Clearance around the pond so nobody lands in the water. */
const POND_CLEARANCE = 60;
/** Landmarks that peers and their trails avoid instead of flattening. */
const STRUCTURES = SCENE.props.filter(
  (p) => p.kind === "house" || p.kind === "garden" || p.kind === "tent" || p.kind === "campfire",
);

/** Trail bend direction from a fingerprint bit, so neighbouring trails fan out. */
function bendOf(fingerprint: string): number {
  return parseInt(fingerprint.slice(8, 9) || "0", 16) % 2 ? 0.14 : -0.14;
}

/** Control point of the trail from the island center to pos. */
function trailCtrl(pos: Pt, bend: number): Pt {
  return [(CX + pos[0]) / 2 - (pos[1] - CY) * bend, (CY + pos[1]) / 2 + (pos[0] - CX) * bend];
}

/** Whether landing at pos, or the trail leading there, would clear a landmark. */
function coversStructure(pos: Pt, bend: number): boolean {
  const trail = quadPoints([CX, CY], trailCtrl(pos, bend), pos, 24);
  return STRUCTURES.some((s) => {
    const nearAvatar = dist([s.x, s.y], pos) < AVATAR_SIZE / 2 + 26 + s.r;
    const underLabel = Math.abs(s.y - (pos[1] + AVATAR_SIZE / 2 + 20)) < 12 + s.r && Math.abs(s.x - pos[0]) < 90 + s.r;
    return nearAvatar || underLabel || trail.some((q) => dist([s.x, s.y], q) < 12 + s.r * 0.8);
  });
}

/** Picks a random spot clear of the pond, landmarks and other peers' labels.
 * On a crowded island it falls back to the roomiest candidate, and only then
 * lets a landmark step aside. */
function randomHome(taken: Pt[], bend: number): Pt {
  let best: Pt = at(R_MAX, Math.random() * 360);
  let bestScore = -Infinity;
  for (let i = 0; i < TRIES; i++) {
    // Area-uniform radius so landings do not bunch toward the center.
    const r = Math.sqrt(R_MIN ** 2 + Math.random() * (R_MAX ** 2 - R_MIN ** 2));
    const p = at(r, Math.random() * 360);
    if (dist(p, SCENE.pond) < POND_CLEARANCE) continue;
    // Gap in label footprints to the nearest peer: 1 or more means no overlap.
    const gap = Math.min(
      Infinity,
      ...taken.map((q) => Math.max(Math.abs(p[0] - q[0]) / MIN_DX, Math.abs(p[1] - q[1]) / MIN_DY)),
    );
    const clearOfLandmarks = !coversStructure(p, bend);
    if (gap >= 1 && clearOfLandmarks) return p;
    // Prefer landmark-safe spots, then the widest gap to other peers.
    const score = (clearOfLandmarks ? 1000 : 0) + Math.min(gap, 999);
    if (score > bestScore) {
      best = p;
      bestScore = score;
    }
  }
  return best;
}

/** Places rendered peers; each keeps its random home until it has left. */
export function usePlacedPeers(rendered: { peer: PeerDto; leaving: boolean }[]): PlacedPeer[] {
  const homes = useRef(new Map<string, Pt>());
  return useMemo(() => {
    const map = homes.current;
    const live = new Set(rendered.map((r) => r.peer.fingerprint));
    for (const fp of [...map.keys()]) {
      if (!live.has(fp)) map.delete(fp);
    }
    for (const { peer } of rendered) {
      const fp = peer.fingerprint;
      if (!map.has(fp)) map.set(fp, randomHome([...map.values()], bendOf(fp)));
    }
    return rendered.map(({ peer, leaving }) => {
      const pos = map.get(peer.fingerprint) ?? at(R_MAX, 0);
      return { peer, leaving, pos, ctrl: trailCtrl(pos, bendOf(peer.fingerprint)), size: AVATAR_SIZE };
    });
  }, [rendered]);
}

/** SVG path of the trail from the island center to the peer. */
export function trailPath(p: PlacedPeer): string {
  return `M${CX} ${CY} Q${p.ctrl[0].toFixed(1)} ${p.ctrl[1].toFixed(1)} ${p.pos[0].toFixed(1)} ${p.pos[1].toFixed(1)}`;
}

/** Props that step aside because a peer, its name label or its trail covers them. */
export function clearedProps(props: SceneProp[], placed: PlacedPeer[]): Set<number> {
  const present = placed.filter((p) => !p.leaving);
  const trail = present.flatMap((p) => quadPoints([CX, CY], p.ctrl, p.pos));
  const cleared = new Set<number>();
  for (const t of props) {
    const onPeer = present.some((o) => {
      if (dist([t.x, t.y], o.pos) < o.size / 2 + 22 + t.r) return true;
      const labelY = o.pos[1] + o.size / 2 + 20;
      const halfLabel = o.peer.name.length * 4 + 32;
      return Math.abs(t.y - labelY) < 12 + t.r && Math.abs(t.x - o.pos[0]) < halfLabel + t.r;
    });
    if (onPeer || trail.some((q) => dist([t.x, t.y], q) < 8 + t.r * 0.8)) cleared.add(t.id);
  }
  return cleared;
}
