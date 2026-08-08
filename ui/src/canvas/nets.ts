// Orthogonal net routing over a computed layout.
//
// Each net gets a vertical "trunk" at the average x of its endpoints
// (plus a stable per-net hash offset so parallel trunks don't overlap),
// and every endpoint connects with an H-then-V L-shape.

import type { SchematicDoc } from "../types";
import type { PinPoint, SchematicLayout } from "./layout";

export type Segment = { x1: number; y1: number; x2: number; y2: number };

export type RoutedNet = {
  name: string;
  kind: string;
  color: string;
  endpoints: PinPoint[];
  segments: Segment[];
  junctions: { x: number; y: number }[];
  /** Where to hang the hover/click label, roughly mid-trunk. */
  labelAt: { x: number; y: number } | null;
};

export function netColor(kind: string): string {
  switch (kind) {
    case "Ground":
      return "#4ade80";
    case "Power":
      return "#f87171";
    case "Analog":
      return "#38bdf8";
    default:
      return "#8b949e";
  }
}

/** Stable string hash (djb2). */
function hash(s: string): number {
  let h = 5381;
  for (let i = 0; i < s.length; i++) {
    h = ((h << 5) + h + s.charCodeAt(i)) | 0;
  }
  return Math.abs(h);
}

export function routeNets(doc: SchematicDoc, layout: SchematicLayout): RoutedNet[] {
  const routed: RoutedNet[] = [];

  for (const net of Object.values(doc.nets)) {
    const endpoints: PinPoint[] = [];
    for (const port of net.ports) {
      const pp = layout.pinPoints.get(`${port.component}:${port.pin}`);
      if (pp) endpoints.push(pp);
    }
    if (endpoints.length === 0) continue;

    const color = netColor(net.kind);
    if (endpoints.length === 1) {
      routed.push({
        name: net.name,
        kind: net.kind,
        color,
        endpoints,
        segments: [],
        junctions: [],
        labelAt: { x: endpoints[0].x, y: endpoints[0].y },
      });
      continue;
    }

    const avgX = endpoints.reduce((s, p) => s + p.x, 0) / endpoints.length;
    const trunkX = Math.round(avgX + (hash(net.name) % 24) - 12);

    const ys = endpoints.map((p) => p.y);
    const minY = Math.min(...ys);
    const maxY = Math.max(...ys);

    const segments: Segment[] = [];
    for (const p of endpoints) {
      if (p.x !== trunkX) {
        segments.push({ x1: p.x, y1: p.y, x2: trunkX, y2: p.y });
      }
    }
    if (maxY > minY) {
      segments.push({ x1: trunkX, y1: minY, x2: trunkX, y2: maxY });
    }

    // Junction dots at T-joins on the trunk (only meaningful for >2 endpoints;
    // the extreme endpoints form corners, not tees).
    const junctions: { x: number; y: number }[] = [];
    if (endpoints.length > 2) {
      const seen = new Set<number>();
      for (const p of endpoints) {
        if (p.y > minY && p.y < maxY && !seen.has(p.y)) {
          seen.add(p.y);
          junctions.push({ x: trunkX, y: p.y });
        }
      }
    }

    routed.push({
      name: net.name,
      kind: net.kind,
      color,
      endpoints,
      segments,
      junctions,
      labelAt: { x: trunkX, y: (minY + maxY) / 2 },
    });
  }

  return routed;
}
