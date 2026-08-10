// Turning a viewer edit event into the position write. Pure on purpose: this
// is the seam the repro harness drives directly (window.__repro), because
// synthetic mouse events can't reliably hit the viewer's SVG in headless
// Chrome — see repro-main.tsx.
import { snapPoint } from "./grid";
import type { BuildView, MoveIn } from "../types";

/** Component instance path -> its current center, read from the view. */
export function centersFromView(view: BuildView): Map<string, { x: number; y: number }> {
  const centers = new Map<string, { x: number; y: number }>();
  for (const el of view.circuit_json) {
    if (el.type !== "schematic_component") continue;
    const id = el.schematic_component_id;
    const path = typeof id === "string" ? view.id_map[id] : undefined;
    const center = el.center as { x: number; y: number } | undefined;
    if (path && center) centers.set(path, center);
  }
  return centers;
}

/**
 * The positions one drop writes. The dragged component lands on the grid; when
 * it is part of a multi-selection the whole set moves by that SNAPPED delta,
 * so the group keeps its shape instead of each member rounding independently.
 * Returns `{}` when the drag target isn't a known component.
 */
export function movesFromEdit(args: {
  draggedPath: string;
  newCenter: { x: number; y: number };
  centers: Map<string, { x: number; y: number }>;
  selection: string[];
}): Record<string, MoveIn> {
  const { draggedPath, newCenter, centers, selection } = args;
  const dropped = snapPoint(newCenter);
  const old = centers.get(draggedPath);
  if (!old) return {};
  const moves: Record<string, MoveIn> = {};
  if (selection.includes(draggedPath) && selection.length > 1) {
    const dx = dropped.x - old.x;
    const dy = dropped.y - old.y;
    for (const path of selection) {
      const c = centers.get(path);
      if (c) moves[path] = { x: c.x + dx, y: c.y + dy };
    }
  } else {
    moves[draggedPath] = dropped;
  }
  return moves;
}
