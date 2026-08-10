/** The schematic-space grid every authored position lands on.
 *
 * One grid for all of it: the placement ghost, drag-to-move, and provisional
 * parts. A schematic reads as aligned only if everything shares a pitch — a
 * part dropped on grid and then nudged off it is what makes derived-looking
 * boards go ragged after the first edit (and forces the router into
 * non-orthogonal runs).
 */
export const SNAP = 0.25;

/** Round one schematic-space coordinate onto the grid. */
export const snap = (v: number) => Math.round(v / SNAP) * SNAP;

/** Round a schematic-space point onto the grid. */
export const snapPoint = (p: { x: number; y: number }) => ({
  x: snap(p.x),
  y: snap(p.y),
});
