// Deterministic schematic layout — a pure function of SchematicDoc.
//
// Hierarchy is walked from "root" via `children`. Only "module" and
// "component" instances become boxes; ports/pins/interfaces are skipped
// (component pin geometry comes from the `pins` array).
//
// Sizing is computed bottom-up (components -> modules -> root), then
// absolute world positions are assigned top-down.

import type { InstanceDoc, PinDoc, SchematicDoc } from "../types";

export type Side = "left" | "right";

export type LayoutBox = {
  path: string;
  name: string; // instance name (last path segment)
  kind: "module" | "component";
  x: number;
  y: number;
  w: number;
  h: number;
  depth: number;
  typeName: string;
  refdes?: string;
  value?: string;
};

export type PinPoint = {
  /** Stub endpoint (the dot), world coords — nets attach here. */
  x: number;
  y: number;
  side: Side;
  /** Attachment point on the box edge (start of the 8px stub). */
  edgeX: number;
  edgeY: number;
  name: string;
  net: string | null;
  componentPath: string;
};

export type SchematicLayout = {
  boxes: Map<string, LayoutBox>;
  /** key = componentPath + ":" + pinName */
  pinPoints: Map<string, PinPoint>;
  bounds: { x: number; y: number; w: number; h: number };
};

export const PIN_STUB = 8;

const COMP_MIN_W = 120;
const COMP_MIN_H = 60;
const PIN_SPACING = 18;
const MODULE_PAD = 24;
const TITLE_H = 22;
const GAP = 48;

// ---------------------------------------------------------------------------

type SizedNode = {
  path: string;
  name: string;
  inst: InstanceDoc;
  kind: "module" | "component";
  w: number;
  h: number;
  children: SizedNode[];
  childOffsets: { x: number; y: number }[]; // parallel to children
  pinSides: { left: PinDoc[]; right: PinDoc[] } | null;
};

function naturalCompare(a: string, b: string): number {
  return a.localeCompare(b, "en", { numeric: true, sensitivity: "base" });
}

const LEFTY = new Set(["1", "A", "P1", "+", "IN", "VIN", "L"]);
const RIGHTY = new Set(["2", "K", "P2", "-", "OUT", "VOUT"]);

function splitPins(pins: PinDoc[]): { left: PinDoc[]; right: PinDoc[] } {
  if (pins.length <= 1) return { left: pins.slice(), right: [] };
  if (pins.length === 2) {
    const score = (p: PinDoc): number => {
      const u = p.name.toUpperCase();
      if (LEFTY.has(u)) return 0;
      if (RIGHTY.has(u)) return 2;
      return 1;
    };
    const sorted = pins
      .slice()
      .sort((a, b) => score(a) - score(b) || naturalCompare(a.name, b.name));
    return { left: [sorted[0]], right: [sorted[1]] };
  }
  const sorted = pins.slice().sort((a, b) => naturalCompare(a.name, b.name));
  const half = Math.ceil(sorted.length / 2);
  return { left: sorted.slice(0, half), right: sorted.slice(half) };
}

function attrString(inst: InstanceDoc, key: string): string | undefined {
  const v = inst.attributes?.[key];
  if (typeof v === "string") return v;
  if (typeof v === "number") return String(v);
  return undefined;
}

/** Crude monospace text-width estimate (px) at ~11px font. */
function textW(s: string | undefined, perChar = 6.8): number {
  return s ? s.length * perChar : 0;
}

function sizeComponent(path: string, name: string, inst: InstanceDoc): SizedNode {
  const pins = inst.pins ?? [];
  const sides = splitPins(pins);
  const maxSide = Math.max(sides.left.length, sides.right.length, 1);
  const h = Math.max(COMP_MIN_H, maxSide * PIN_SPACING + 26);

  const value = attrString(inst, "value");
  const labelW = Math.max(
    textW(inst.refdes, 7.5),
    textW(inst.type_name),
    textW(value),
  );
  const maxLeftPin = sides.left.reduce((m, p) => Math.max(m, p.name.length), 0);
  const maxRightPin = sides.right.reduce((m, p) => Math.max(m, p.name.length), 0);
  const pinClearW = (maxLeftPin + maxRightPin) * 6 + 44;
  const w = Math.max(COMP_MIN_W, labelW + 28, pinClearW);

  return {
    path,
    name,
    inst,
    kind: "component",
    w,
    h,
    children: [],
    childOffsets: [],
    pinSides: sides,
  };
}

function drawableChildren(
  doc: SchematicDoc,
  inst: InstanceDoc,
): { name: string; path: string; inst: InstanceDoc }[] {
  const out: { name: string; path: string; inst: InstanceDoc }[] = [];
  for (const [name, childPath] of Object.entries(inst.children ?? {})) {
    const child = doc.instances[childPath];
    if (!child) continue;
    if (child.kind === "component" || child.kind === "module") {
      out.push({ name, path: childPath, inst: child });
    }
  }
  // Components first, then submodules; stable natural order within each group.
  out.sort((a, b) => {
    const ga = a.inst.kind === "component" ? 0 : 1;
    const gb = b.inst.kind === "component" ? 0 : 1;
    return ga - gb || naturalCompare(a.name, b.name);
  });
  return out;
}

function sizeModule(
  doc: SchematicDoc,
  path: string,
  name: string,
  inst: InstanceDoc,
): SizedNode {
  const kids = drawableChildren(doc, inst).map((c) => sizeNode(doc, c.path, c.name, c.inst));

  // Pack children into rows targeting a near-square aspect.
  const n = kids.length;
  const cols = Math.max(1, Math.ceil(Math.sqrt(n)));
  const offsets: { x: number; y: number }[] = [];
  let innerW = 0;
  let cursorY = 0;
  for (let r = 0; r * cols < n; r++) {
    const row = kids.slice(r * cols, (r + 1) * cols);
    const rowH = Math.max(...row.map((k) => k.h));
    let cursorX = 0;
    for (const kid of row) {
      // Center each child vertically within its row.
      offsets.push({ x: cursorX, y: cursorY + (rowH - kid.h) / 2 });
      cursorX += kid.w + GAP;
    }
    innerW = Math.max(innerW, cursorX - GAP);
    cursorY += rowH + GAP;
  }
  const innerH = n > 0 ? cursorY - GAP : 0;

  const titleW = textW(name, 7.5) + textW(inst.type_name) + 40;
  const w = Math.max(innerW + 2 * MODULE_PAD, titleW, 160);
  const h = TITLE_H + MODULE_PAD + Math.max(innerH, 20) + MODULE_PAD;

  return {
    path,
    name,
    inst,
    kind: "module",
    w,
    h,
    children: kids,
    childOffsets: offsets,
    pinSides: null,
  };
}

function sizeNode(doc: SchematicDoc, path: string, name: string, inst: InstanceDoc): SizedNode {
  return inst.kind === "component"
    ? sizeComponent(path, name, inst)
    : sizeModule(doc, path, name, inst);
}

function place(node: SizedNode, x: number, y: number, depth: number, out: SchematicLayout): void {
  const isRoot = depth < 0;
  if (!isRoot) {
    out.boxes.set(node.path, {
      path: node.path,
      name: node.name,
      kind: node.kind,
      x,
      y,
      w: node.w,
      h: node.h,
      depth,
      typeName: node.inst.type_name,
      refdes: node.inst.refdes,
      value: attrString(node.inst, "value"),
    });
  }

  if (node.kind === "component" && node.pinSides) {
    const placeSide = (pins: PinDoc[], side: Side) => {
      const nPins = pins.length;
      pins.forEach((pin, i) => {
        const py = y + (node.h * (i + 1)) / (nPins + 1);
        const edgeX = side === "left" ? x : x + node.w;
        const dotX = side === "left" ? edgeX - PIN_STUB : edgeX + PIN_STUB;
        out.pinPoints.set(`${node.path}:${pin.name}`, {
          x: dotX,
          y: py,
          side,
          edgeX,
          edgeY: py,
          name: pin.name,
          net: pin.net ?? null,
          componentPath: node.path,
        });
      });
    };
    placeSide(node.pinSides.left, "left");
    placeSide(node.pinSides.right, "right");
    return;
  }

  const originX = isRoot ? x : x + MODULE_PAD;
  const originY = isRoot ? y : y + TITLE_H + MODULE_PAD;
  node.children.forEach((child, i) => {
    const off = node.childOffsets[i];
    place(child, originX + off.x, originY + off.y, depth + 1, out);
  });
}

export function layoutSchematic(doc: SchematicDoc): SchematicLayout {
  const out: SchematicLayout = {
    boxes: new Map(),
    pinPoints: new Map(),
    bounds: { x: 0, y: 0, w: 0, h: 0 },
  };
  const root = doc.instances["root"];
  if (!root) return out;

  // The root is laid out like a module but not drawn (depth -1).
  const sized = sizeModule(doc, "root", "root", root);
  place(sized, 0, 0, -1, out);

  let minX = Infinity;
  let minY = Infinity;
  let maxX = -Infinity;
  let maxY = -Infinity;
  for (const b of out.boxes.values()) {
    minX = Math.min(minX, b.x - PIN_STUB);
    minY = Math.min(minY, b.y);
    maxX = Math.max(maxX, b.x + b.w + PIN_STUB);
    maxY = Math.max(maxY, b.y + b.h);
  }
  if (out.boxes.size === 0) {
    minX = minY = 0;
    maxX = maxY = 0;
  }
  out.bounds = { x: minX, y: minY, w: maxX - minX, h: maxY - minY };
  return out;
}
