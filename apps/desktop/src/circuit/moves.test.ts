// What a drop writes. These cover the arithmetic that every move gesture funnels
// through — drag-to-move, arrow nudge, group drag — so a regression here shows up
// as a failing test instead of a board that quietly drifts off-grid.
//
// The gestures themselves (does the symbol track the cursor? does a group keep
// its shape? does releasing a drag preserve the selection?) live in the viewer
// and cannot be reached from here: see tools/gesture-check.mjs.
import { describe, expect, it } from "vitest";

import { SNAP, snap, snapPoint } from "./grid";
import { centersFromView, movesFromEdit } from "./moves";
import type { BuildView } from "../types";

const onGrid = (v: number) => Math.abs(v / SNAP - Math.round(v / SNAP)) < 1e-9;

/** A view with components at the given schematic centers. */
function viewWith(centers: Record<string, { x: number; y: number }>): BuildView {
  const elements: BuildView["circuit_json"] = [];
  const id_map: Record<string, string> = {};
  for (const [path, c] of Object.entries(centers)) {
    const id = `sch:${path}`;
    id_map[id] = path;
    elements.push({
      type: "schematic_component",
      schematic_component_id: id,
      center: c,
      size: { width: 0.9, height: 0.6 },
    } as unknown as BuildView["circuit_json"][number]);
  }
  return {
    version: 4,
    source: "board.zen",
    schematic: null,
    diagnostics: [],
    circuit_json: elements,
    id_map,
    source_hash: "hash",
    editability: null,
  };
}

describe("grid", () => {
  it("rounds to the nearest grid step, including negatives", () => {
    expect(snap(1.37)).toBe(1.25);
    expect(snap(-2.06)).toBe(-2);
    expect(snap(0)).toBe(0);
    expect(snapPoint({ x: 1.37, y: -2.06 })).toEqual({ x: 1.25, y: -2 });
  });

  it("leaves values already on the grid untouched", () => {
    for (const v of [-1, -0.25, 0, 0.5, 3.75]) expect(snap(v)).toBe(v);
  });
});

describe("centersFromView", () => {
  it("keys centers by instance path via id_map, never by parsing ids", () => {
    const view = viewWith({ "root.A.R": { x: 1, y: -2 }, "root.B.R": { x: 3, y: -4 } });
    const centers = centersFromView(view);
    expect([...centers.keys()].sort()).toEqual(["root.A.R", "root.B.R"]);
    expect(centers.get("root.A.R")).toEqual({ x: 1, y: -2 });
  });

  it("ignores elements that are not components", () => {
    const view = viewWith({ "root.A.R": { x: 1, y: -2 } });
    view.circuit_json.push({
      type: "schematic_trace",
      schematic_trace_id: "schtrace:N",
    } as unknown as BuildView["circuit_json"][number]);
    expect(centersFromView(view).size).toBe(1);
  });
});

describe("movesFromEdit", () => {
  const centers = () =>
    centersFromView(
      viewWith({
        "root.A.R": { x: 0.95, y: -1.3376 },
        "root.B.R": { x: 0.95, y: -2.4876 },
        "root.C.R": { x: 4, y: -1 },
      }),
    );

  it("snaps a single drop onto the grid", () => {
    const moves = movesFromEdit({
      draggedPath: "root.A.R",
      newCenter: { x: 1.37, y: -2.06 },
      centers: centers(),
      selection: [],
    });
    expect(moves).toEqual({ "root.A.R": { x: 1.25, y: -2 } });
  });

  it("moves only the dragged component when it is not part of a multi-selection", () => {
    const moves = movesFromEdit({
      draggedPath: "root.A.R",
      newCenter: { x: 2, y: -2 },
      centers: centers(),
      selection: ["root.B.R", "root.C.R"],
    });
    expect(Object.keys(moves)).toEqual(["root.A.R"]);
  });

  it("moves a whole selection by the SNAPPED delta, preserving its shape", () => {
    const before = centers();
    const gap = {
      x: before.get("root.B.R")!.x - before.get("root.A.R")!.x,
      y: before.get("root.B.R")!.y - before.get("root.A.R")!.y,
    };
    const moves = movesFromEdit({
      draggedPath: "root.A.R",
      newCenter: { x: 1.31, y: -2.09 },
      centers: before,
      selection: ["root.A.R", "root.B.R"],
    });
    expect(Object.keys(moves).sort()).toEqual(["root.A.R", "root.B.R"]);
    // The grabbed one lands on the grid…
    expect(onGrid(moves["root.A.R"].x)).toBe(true);
    expect(onGrid(moves["root.A.R"].y)).toBe(true);
    // …and the set keeps its exact internal offsets, so a group never distorts.
    expect(moves["root.B.R"].x - moves["root.A.R"].x).toBeCloseTo(gap.x, 10);
    expect(moves["root.B.R"].y - moves["root.A.R"].y).toBeCloseTo(gap.y, 10);
  });

  it("leaves unselected components alone during a group drag", () => {
    const moves = movesFromEdit({
      draggedPath: "root.A.R",
      newCenter: { x: 2, y: -2 },
      centers: centers(),
      selection: ["root.A.R", "root.B.R"],
    });
    expect(moves["root.C.R"]).toBeUndefined();
  });

  it("writes nothing when the drag target is unknown", () => {
    expect(
      movesFromEdit({
        draggedPath: "root.NOPE.R",
        newCenter: { x: 1, y: 1 },
        centers: centers(),
        selection: [],
      }),
    ).toEqual({});
  });

  it("is idempotent: re-dropping an on-grid component changes nothing", () => {
    const c = centersFromView(viewWith({ "root.A.R": { x: 1.25, y: -2 } }));
    const moves = movesFromEdit({
      draggedPath: "root.A.R",
      newCenter: { x: 1.25, y: -2 },
      centers: c,
      selection: [],
    });
    expect(moves["root.A.R"]).toEqual({ x: 1.25, y: -2 });
  });
});
