// The schematic canvas: @tscircuit/schematic-viewer fed by the Circuit JSON
// view-model from the build payload. This is the only module that imports
// tscircuit packages (see docs/decisions/0001-circuit-json-renderer.md).
//
// Selection flow: viewer click -> circuit-json id -> id_map -> instance path
// -> selection store -> set_selection command. Net labels have no viewer
// callback, so a delegated DOM click handler resolves
// [data-schematic-net-label-id] through the same id_map. Selection and
// diagnostic highlighting are injected as per-id CSS through the viewer's
// `css` prop.

import { useEffect, useMemo, useRef, useState } from "react";
import type { ComponentProps } from "react";
import { SchematicViewer } from "@tscircuit/schematic-viewer";
import { Button, IconCornersOut, IconMinus, IconPlus } from "@etchable/ui";
import { observeCamera, readCamera } from "./camera";
import { humanizeError, type FriendlyError } from "./errors";
import GestureOverlay from "./GestureOverlay";
import InlinePrompt from "./InlinePrompt";
import PlacementLayer from "./PlacementLayer";
import ProvisionalLayer, { type Provisional } from "./ProvisionalLayer";
import type {
  BuildView,
  ConnectOutcome,
  Diag,
  LabelArm,
  PlacementArm,
  MoveIn,
} from "../types";

// The viewer bundles its own circuit-json types (a different version than
// our pin), so target its actual prop type rather than either package's.
type ViewerCircuitJson = ComponentProps<typeof SchematicViewer>["circuitJson"];
type CameraApiRef = NonNullable<ComponentProps<typeof SchematicViewer>["cameraApiRef"]>;

const ZOOM_STEP = 1.25;

const ACCENT = "#4d9fff";
const ERROR = "#d64545";
const WARNING = "#c9950c";

type CircuitCanvasProps = {
  view: BuildView | null;
  source: string | null;
  dimmed: boolean;
  diagnostics: Diag[];
  selection: string[];
  onSelectionChange: (paths: string[]) => void;
  onSavePositions: (moves: Record<string, MoveIn>, baseHash: string) => void;
  /** Armed palette item — the placement layer takes the canvas over. */
  placement: PlacementArm | null;
  onPlacementCommit: (
    name: string,
    attrs: [string, string][],
    position: { x: number; y: number; rotation: number },
  ) => Promise<void>;
  onPlacementFinish: () => void;
  /** Armed net-label tool — pin clicks attach instead of selecting. */
  labelMode: LabelArm | null;
  onLabelFinish: () => void;
  onAttachPin: (path: string, pin: string, netName: string, kind: string) => Promise<void>;
  onRenameNet: (from: string, to: string) => Promise<void>;
  /** Wire two pins (phase 3); needs_merge comes back for confirmation. */
  onConnectPins: (
    a: { path: string; pin: string },
    b: { path: string; pin: string },
    allowMerge: boolean,
  ) => Promise<ConnectOutcome>;
  /** Gesture undo/redo (gate snapshots); resolves to the gesture's label. */
  onUndo: () => Promise<string>;
  onRedo: () => Promise<string>;
  /** Phase 4 manipulation verbs. */
  onSetAttribute: (path: string, key: string, value: string) => Promise<void>;
  onRenameInstance: (from: string, to: string) => Promise<void>;
  onRemoveInstances: (paths: string[]) => Promise<void>;
  /** Refusal handoff: send a pre-filled message to the agent chat. */
  onAskAgent: (text: string) => void;
  /** The LATEST build's hash (current file), also when the board is red. */
  latestHash: string | null;
  /** Placed parts not yet in a clean build — rendered as stand-ins. */
  provisionals: Provisional[];
  onProvisionalMoved: (name: string, x: number, y: number) => void;
};

// The viewer's drag events, typed structurally — @tscircuit/props is a
// transitive dep we deliberately don't declare (see docs/decisions/0001).
type SchematicEditEvent = {
  edit_event_type?: string;
  schematic_component_id?: string;
  new_center?: { x: number; y: number };
  in_progress?: boolean;
};

function fileMatches(a: string | null | undefined, b: string | undefined): boolean {
  if (!a || !b) return false;
  return a === b || a.endsWith("/" + b) || b.endsWith("/" + a);
}

/** CSS-escape an attribute value for use in a selector string. */
function cssAttr(value: string): string {
  return value.replace(/["\\]/g, "\\$&");
}

// The viewer memoizes its SVG on circuitJson.length + editCount, not array
// identity; rebuilds that keep the element count would render stale geometry
// without this bump.
let editSeq = 0;
const stamped = new WeakSet<object>();
function withEditCount(elements: BuildView["circuit_json"]): ViewerCircuitJson {
  if (!stamped.has(elements)) {
    stamped.add(elements);
    (elements as unknown as { editCount: number }).editCount = ++editSeq;
  }
  // The wire payload is opaque JSON; the Rust emitter + the zod validation
  // harness own schema conformance, so this cast is the module boundary.
  return elements as unknown as ViewerCircuitJson;
}

export default function CircuitCanvas(props: CircuitCanvasProps) {
  const {
    view,
    source,
    dimmed,
    diagnostics,
    selection,
    onSelectionChange,
    onSavePositions,
    placement,
    onPlacementCommit,
    onPlacementFinish,
    labelMode,
    onLabelFinish,
    onAttachPin,
    onRenameNet,
    onConnectPins,
    onUndo,
    onRedo,
    onSetAttribute,
    onRenameInstance,
    onRemoveInstances,
    onAskAgent,
    latestHash,
    provisionals,
    onProvisionalMoved,
  } = props;
  const placementRef = useRef(placement);
  placementRef.current = placement;
  const labelModeRef = useRef(labelMode);
  labelModeRef.current = labelMode;
  const onLabelFinishRef = useRef(onLabelFinish);
  onLabelFinishRef.current = onLabelFinish;
  const onUndoRef = useRef(onUndo);
  onUndoRef.current = onUndo;
  const onRedoRef = useRef(onRedo);
  onRedoRef.current = onRedo;
  const viewRef = useRef(view);
  viewRef.current = view;
  const onRemoveRef = useRef(onRemoveInstances);
  onRemoveRef.current = onRemoveInstances;
  const onSaveRef = useRef(onSavePositions);
  onSaveRef.current = onSavePositions;

  // Delete needs a confirmation when it cascades (>3 instances).
  const [deleteConfirm, setDeleteConfirm] = useState<{
    paths: string[];
    busy: boolean;
  } | null>(null);

  /** The selection's deletable instance paths (nets filtered out). */
  const deletableSelection = () => {
    const v = viewRef.current;
    if (!v?.editability) return [];
    return selectionRef.current.filter((p) => v.editability?.instances[p]);
  };

  const removeSelection = (paths: string[]) => {
    setDeleteConfirm(null);
    onRemoveRef
      .current(paths)
      .then(() => onSelectRef.current([]))
      .catch((err) =>
        showToastErrRef.current(err, {
          retry: () => removeSelection(paths),
          intent: `Please delete ${paths.join(", ")}`,
        }),
      );
  };

  const latestHashRef = useRef(latestHash);
  latestHashRef.current = latestHash;

  const rotateSelection = () => {
    const v = viewRef.current;
    const hash = latestHashRef.current;
    if (!v || !hash) return;
    const moves: Record<string, MoveIn> = {};
    for (const el of v.circuit_json) {
      if (el.type !== "schematic_component") continue;
      const id = el.schematic_component_id;
      const path = typeof id === "string" ? v.id_map[id] : undefined;
      const c = el.center as { x: number; y: number } | undefined;
      if (path && c && selectionRef.current.includes(path)) {
        moves[path] = { x: c.x, y: c.y, rotate_by: 90 };
      }
    }
    if (Object.keys(moves).length > 0) onSaveRef.current(moves, hash);
  };

  // Inline prompts: label-a-pin, rename-a-net (phase 2), edit-a-value and
  // rename-an-instance (phase 4, via double-click on a component).
  const [prompt, setPrompt] = useState<
    | { kind: "label"; path: string; pin: string; screen: { x: number; y: number } }
    | { kind: "rename"; net: string; screen: { x: number; y: number } }
    | { kind: "value"; path: string; current: string; screen: { x: number; y: number } }
    | { kind: "iname"; from: string; screen: { x: number; y: number } }
    | null
  >(null);
  // Toasts speak user voice; the raw writer/gate string rides along as a
  // tooltip for the users who do read source (UX review P0-4). Stale
  // rejections carry a retry (the writers' name-anchored resolution IS the
  // re-validation, PRD §8); structural refusals carry the agent handoff.
  const [toast, setToast] = useState<{
    fe: FriendlyError;
    retry?: () => void;
    /** The gesture in words, for the pre-filled chat message. */
    intent?: string;
  } | null>(null);
  const toastTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const armToast = (t: NonNullable<typeof toast>, ms: number) => {
    setToast(t);
    if (toastTimer.current) clearTimeout(toastTimer.current);
    toastTimer.current = setTimeout(() => setToast(null), ms);
  };
  const showToast = (raw: unknown, opts?: { retry?: () => void; intent?: string }) => {
    const fe = humanizeError(raw);
    // Actionable toasts stick around longer.
    const actionable = (fe.kind === "stale" && opts?.retry) || (fe.kind === "refusal" && opts?.intent);
    armToast({ fe, ...opts }, actionable ? 10000 : 5000);
  };
  const showToastText = (message: string) => {
    armToast({ fe: { message, detail: message, kind: "other" } }, 4000);
  };
  const showToastRef = useRef(showToastText);
  showToastRef.current = showToastText;
  const showToastErrRef = useRef(showToast);
  showToastErrRef.current = showToast;

  // A wire drag that requires a net merge — confirmed, never silent
  // (PRD §12.4). Anchored at the gesture that raised it.
  const [mergeConfirm, setMergeConfirm] = useState<{
    a: { path: string; pin: string };
    b: { path: string; pin: string };
    from: string;
    into: string;
    fromRefs: number;
    screen: { x: number; y: number };
    busy: boolean;
  } | null>(null);

  const wireConnect = async (
    a: { path: string; pin: string },
    b: { path: string; pin: string },
    allowMerge: boolean,
    screen: { x: number; y: number },
  ) => {
    try {
      const outcome = await onConnectPins(a, b, allowMerge);
      if (outcome.outcome === "needs_merge") {
        setMergeConfirm({
          a,
          b,
          from: outcome.from,
          into: outcome.into,
          fromRefs: outcome.from_refs,
          screen,
          busy: false,
        });
      } else {
        setMergeConfirm(null);
        if (outcome.already) showToastText(`Already connected on ${outcome.net}.`);
        else if (outcome.via_port)
          showToastText(`Wired through ${outcome.via_port} — that pin reaches the board as a port.`);
      }
    } catch (err) {
      setMergeConfirm(null);
      showToast(err, {
        retry: () => void wireConnect(a, b, allowMerge, screen),
        intent: `Please connect ${a.path} pin ${a.pin} to ${b.path} pin ${b.pin}`,
      });
    }
  };

  const selectionRef = useRef(selection);
  selectionRef.current = selection;
  const onSelectRef = useRef(onSelectionChange);
  onSelectRef.current = onSelectionChange;
  // Set when a viewer callback handled the click, so the container's
  // background handler (which the same click bubbles into) doesn't clear it.
  const clickHandledRef = useRef(false);
  const wrapRef = useRef<HTMLDivElement>(null);

  // The paper dot grid lives on the container as a CSS background; mirror
  // the viewer's camera onto it (adapting the cell by powers of 5 to keep a
  // sane density) so the grid pans and zooms with the board instead of
  // sitting behind it as static wallpaper. The camera mirroring itself is
  // shared with the gesture overlay — see camera.ts.
  //
  // Camera stability across refits (container resizes, rebuild echoes) is
  // NOT handled here anymore: the vendored viewer patch
  // (patches/@tscircuit__schematic-viewer.patch) folds every fit change into
  // the viewer's own pan/zoom matrix, so the schematic stays locked in place
  // and this observer only has to keep the grid glued to it.
  useEffect(() => {
    const wrap = wrapRef.current;
    if (!wrap) return;
    return observeCamera(wrap, ({ a, e, f }) => {
      let cell = Math.abs(a); // px per schematic unit
      while (cell < 18) cell *= 5;
      while (cell > 220) cell /= 5;
      wrap.style.backgroundSize = `${cell}px ${cell}px`;
      wrap.style.backgroundPosition = `${e}px ${f}px`;
    });
  }, [source]);

  // Marquee select (phase 4): shift+drag on the canvas draws a rubber
  // band and selects the components whose centers fall inside. The
  // capture-phase mousedown keeps the viewer's pan from starting.
  const marqueeRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const wrap = wrapRef.current;
    if (!wrap) return;
    let start: { x: number; y: number } | null = null;
    const down = (e: MouseEvent) => {
      if (!e.shiftKey || e.button !== 0) return;
      if ((e.target as Element).closest?.(".pin-hit")) return; // shift-click select
      e.preventDefault();
      e.stopPropagation();
      const rect = wrap.getBoundingClientRect();
      start = { x: e.clientX - rect.left, y: e.clientY - rect.top };
    };
    const move = (e: MouseEvent) => {
      const box = marqueeRef.current;
      if (!start || !box) return;
      const rect = wrap.getBoundingClientRect();
      const x = e.clientX - rect.left;
      const y = e.clientY - rect.top;
      box.style.display = "block";
      box.style.left = `${Math.min(start.x, x)}px`;
      box.style.top = `${Math.min(start.y, y)}px`;
      box.style.width = `${Math.abs(x - start.x)}px`;
      box.style.height = `${Math.abs(y - start.y)}px`;
    };
    const up = (e: MouseEvent) => {
      const box = marqueeRef.current;
      if (!start || !box) return;
      const rect = wrap.getBoundingClientRect();
      const x0 = Math.min(start.x, e.clientX - rect.left);
      const x1 = Math.max(start.x, e.clientX - rect.left);
      const y0 = Math.min(start.y, e.clientY - rect.top);
      const y1 = Math.max(start.y, e.clientY - rect.top);
      start = null;
      box.style.display = "none";
      if (x1 - x0 < 4 && y1 - y0 < 4) return; // a click, not a drag
      clickHandledRef.current = true; // don't let the click clear it
      const v = viewRef.current;
      const cam = readCamera(wrap);
      if (!v || !cam) return;
      const hit: string[] = [];
      for (const el of v.circuit_json) {
        if (el.type !== "schematic_component") continue;
        const id = el.schematic_component_id;
        const path = typeof id === "string" ? v.id_map[id] : undefined;
        const c = el.center as { x: number; y: number } | undefined;
        if (!path || !c) continue;
        const sx = cam.a * c.x + cam.e;
        const sy = cam.d * c.y + cam.f;
        if (sx >= x0 && sx <= x1 && sy >= y0 && sy <= y1) hit.push(path);
      }
      if (hit.length > 0) onSelectRef.current(hit);
    };
    wrap.addEventListener("mousedown", down, true);
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
    return () => {
      wrap.removeEventListener("mousedown", down, true);
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
    };
  }, []);

  // path/net -> toggled selection update, shared by all click paths.
  const applyClick = (target: string, shiftKey: boolean) => {
    const current = selectionRef.current;
    if (shiftKey) {
      onSelectRef.current(
        current.includes(target)
          ? current.filter((p) => p !== target)
          : [...current, target],
      );
    } else {
      onSelectRef.current([target]);
    }
  };

  // The patched viewer hands us an imperative camera (zoom/fit/getScale).
  const cameraRef = useRef<CameraApiRef["current"]>(null);

  // Keyboard: Escape clears the selection; +/- zoom at center; f or 0
  // fits the whole board (the recovery action — the camera never refits
  // on its own). All skipped while typing in an input.
  useEffect(() => {
    const isTyping = () => {
      const el = document.activeElement;
      return (
        el !== null &&
        (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.tagName === "BUTTON" ||
          el.tagName === "SELECT")
      );
    };
    const down = (e: KeyboardEvent) => {
      if (isTyping()) return;
      // Gesture undo/redo (gate snapshots) — the toast names the gesture.
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "z" && !e.altKey) {
        e.preventDefault();
        const redo = e.shiftKey;
        void (redo ? onRedoRef.current() : onUndoRef.current())
          .then((label) => showToastRef.current(`${redo ? "Redid" : "Undid"} ${label}.`))
          .catch((err) => showToastErrRef.current(err));
        return;
      }
      if (e.metaKey || e.ctrlKey || e.altKey) return;
      // While placing, the placement layer owns the keyboard (Esc cancels
      // placement in the capture phase; R rotates).
      if (placementRef.current) return;
      // Esc ends an armed label tool before it clears the selection.
      if (e.key === "Escape" && labelModeRef.current) {
        onLabelFinishRef.current();
        return;
      }
      if (e.key === "Escape" && selectionRef.current.length > 0) {
        onSelectRef.current([]);
      } else if (e.key === "Delete" || e.key === "Backspace") {
        const paths = deletableSelection();
        if (paths.length === 0) return;
        e.preventDefault();
        if (paths.length > 3) setDeleteConfirm({ paths, busy: false });
        else removeSelection(paths);
      } else if ((e.key === "r" || e.key === "R") && selectionRef.current.length > 0) {
        rotateSelection();
      } else if (e.key === "+" || e.key === "=") {
        cameraRef.current?.zoom(ZOOM_STEP);
      } else if (e.key === "-") {
        cameraRef.current?.zoom(1 / ZOOM_STEP);
      } else if (e.key === "f" || e.key === "0") {
        cameraRef.current?.fit();
      }
    };
    window.addEventListener("keydown", down);
    return () => window.removeEventListener("keydown", down);
  }, []);

  // Reverse id_map indexes for highlight CSS: instance path -> component id,
  // net name -> net label ids / trace ids.
  const reverse = useMemo(() => {
    const componentIdByPath = new Map<string, string>();
    const labelIdsByNet = new Map<string, string[]>();
    const traceIdsByNet = new Map<string, string[]>();
    const add = (map: Map<string, string[]>, key: string, id: string) => {
      const list = map.get(key);
      if (list) list.push(id);
      else map.set(key, [id]);
    };
    if (view) {
      for (const [id, target] of Object.entries(view.id_map)) {
        if (id.startsWith("sch:")) componentIdByPath.set(target, id);
        else if (id.startsWith("netlabel:")) add(labelIdsByNet, target, id);
        else if (id.startsWith("schtrace:")) add(traceIdsByNet, target, id);
      }
    }
    return { componentIdByPath, labelIdsByNet, traceIdsByNet };
  }, [view]);

  // Diagnostics -> instance paths (via source_file), for highlight CSS.
  const diagSeverityByPath = useMemo(() => {
    const m = new Map<string, "error" | "warning">();
    if (!view?.schematic) return m;
    const active = diagnostics.filter(
      (d) => !d.suppressed && (d.severity === "error" || d.severity === "warning"),
    );
    if (active.length === 0) return m;
    for (const inst of Object.values(view.schematic.instances)) {
      if (inst.kind !== "component") continue;
      const hits = active.filter((d) => fileMatches(inst.source_file, d.file));
      if (hits.length === 0) continue;
      m.set(inst.path, hits.some((d) => d.severity === "error") ? "error" : "warning");
    }
    return m;
  }, [view, diagnostics]);

  const highlightCss = useMemo(() => {
    const rules: string[] = [];
    for (const [path, severity] of diagSeverityByPath) {
      const id = reverse.componentIdByPath.get(path);
      if (!id) continue;
      const color = severity === "error" ? ERROR : WARNING;
      rules.push(
        `g[data-schematic-component-id="${cssAttr(id)}"] { filter: drop-shadow(0 0 4px ${color}); }`,
      );
    }
    for (const target of selection) {
      const compId = reverse.componentIdByPath.get(target);
      if (compId) {
        rules.push(
          `g[data-schematic-component-id="${cssAttr(compId)}"] { filter: drop-shadow(0 0 5px ${ACCENT}); }`,
        );
      }
      for (const labelId of reverse.labelIdsByNet.get(target) ?? []) {
        rules.push(
          `[data-schematic-net-label-id="${cssAttr(labelId)}"] { filter: drop-shadow(0 0 5px ${ACCENT}); }`,
        );
      }
      for (const traceId of reverse.traceIdsByNet.get(target) ?? []) {
        rules.push(
          `g[data-schematic-trace-id="${cssAttr(traceId)}"] { filter: drop-shadow(0 0 5px ${ACCENT}); }`,
        );
      }
    }
    return rules.join("\n");
  }, [selection, reverse, diagSeverityByPath]);

  // Delegated clicks: net labels and routed traces (no viewer callback
  // exists for either; traces carry an oversized invisible hitbox path) and
  // background (clear selection). Component clicks arrive via the viewer
  // callback and set clickHandledRef before bubbling here.
  const onContainerClick = (e: React.MouseEvent<HTMLDivElement>) => {
    if (clickHandledRef.current) {
      clickHandledRef.current = false;
      return;
    }
    if (!view) return;
    for (const attr of ["data-schematic-net-label-id", "data-schematic-trace-id"]) {
      const el = (e.target as Element).closest?.(`[${attr}]`);
      const id = el?.getAttribute(attr);
      const net = id ? view.id_map[id] : undefined;
      if (net) {
        applyClick(net, e.shiftKey);
        return;
      }
    }
    if (!e.shiftKey && selectionRef.current.length > 0) onSelectRef.current([]);
  };

  // Double-click: a net label renames the net (phase 2); a component edits
  // its value, or renames the instance when it has no value attr (phase
  // 4). Editability gates both with the honest reason.
  const onContainerDoubleClick = (e: React.MouseEvent<HTMLDivElement>) => {
    if (!view) return;
    const labelEl = (e.target as Element).closest?.("[data-schematic-net-label-id]");
    const labelId = labelEl?.getAttribute("data-schematic-net-label-id");
    const net = labelId ? view.id_map[labelId] : undefined;
    if (net) {
      const edit = view.editability?.nets[net];
      if (edit && !edit.editable) {
        showToast(edit.reason ?? `${net} cannot be renamed structurally`, {
          intent: `Please rename the net ${net}`,
        });
        return;
      }
      setPrompt({ kind: "rename", net, screen: { x: e.clientX, y: e.clientY } });
      return;
    }
    const compEl = (e.target as Element).closest?.("[data-schematic-component-id]");
    const compId = compEl?.getAttribute("data-schematic-component-id");
    const path = compId ? view.id_map[compId] : undefined;
    if (!path) return;
    const edit = view.editability?.instances[path];
    if (edit && !edit.editable && !edit.anchor) {
      showToast(edit.reason ?? `${path} is generated and can't be edited directly`, {
        intent: `Please edit ${path} for me`,
      });
      return;
    }
    const value = view.schematic?.instances[path]?.attributes?.value;
    if (value !== undefined) {
      setPrompt({
        kind: "value",
        path,
        current: String(value),
        screen: { x: e.clientX, y: e.clientY },
      });
    } else {
      const anchor = edit && !edit.editable ? edit.anchor : path;
      const from = (anchor ?? path).split(".").pop();
      if (!from) return;
      setPrompt({ kind: "iname", from, screen: { x: e.clientX, y: e.clientY } });
    }
  };

  const selChipText = useMemo(() => {
    if (selection.length === 0) return "";
    const joined = selection.join(", ");
    return joined.length > 64 ? joined.slice(0, 64) + "…" : joined;
  }, [selection]);

  // Non-editable selections say so at selection time, not failure time.
  const selGenerated = useMemo(() => {
    if (!view?.editability) return false;
    return selection.some((p) => {
      const e = view.editability?.instances[p];
      return e ? !e.editable && !e.anchor : false;
    });
  }, [selection, view]);

  // One-time discoverability hint for the wire gesture: shown the first
  // time a board with at least two components is open.
  const [wireHint, setWireHint] = useState(false);
  useEffect(() => {
    if (!view || localStorage.getItem("etchable.wire-hint")) return;
    const comps = view.circuit_json.filter((e) => e.type === "schematic_component").length;
    if (comps < 2) return;
    setWireHint(true);
    localStorage.setItem("etchable.wire-hint", "1");
    const t = setTimeout(() => setWireHint(false), 8000);
    return () => clearTimeout(t);
  }, [view]);

  // Drag-to-move: the viewer fires one event per drag (on mouseup) with the
  // new center in schematic coordinates. Send PARTIAL moves — the backend
  // merges the save-all map the all-or-nothing authored rule needs, keeping
  // derived orientations for everything untouched. Dragging a member of a
  // multi-selection moves the whole selection by the same delta (the other
  // members snap on the rebuild — the rebuild is the confirmation).
  // While a member of a multi-selection is being dragged, show dashed
  // destination outlines for the other members (they commit on the
  // rebuild; the ghosts let the user see it before it lands).
  const groupGhostsRef = useRef<HTMLDivElement>(null);
  const updateGroupGhosts = (event: SchematicEditEvent) => {
    const host = groupGhostsRef.current;
    const wrap = wrapRef.current;
    if (!host || !wrap || !view) return;
    const draggedPath = event.schematic_component_id
      ? view.id_map[event.schematic_component_id]
      : undefined;
    const sel = selectionRef.current;
    if (!draggedPath || !sel.includes(draggedPath) || sel.length < 2 || !event.new_center) {
      host.replaceChildren();
      return;
    }
    const cam = readCamera(wrap);
    if (!cam) return;
    const geo = new Map<string, { x: number; y: number; w: number; h: number }>();
    for (const el of view.circuit_json) {
      if (el.type !== "schematic_component") continue;
      const id = el.schematic_component_id;
      const path = typeof id === "string" ? view.id_map[id] : undefined;
      const c = el.center as { x: number; y: number } | undefined;
      const s = el.size as { width: number; height: number } | undefined;
      if (path && c) geo.set(path, { x: c.x, y: c.y, w: s?.width ?? 1, h: s?.height ?? 0.6 });
    }
    const orig = geo.get(draggedPath);
    if (!orig) return;
    const dx = event.new_center.x - orig.x;
    const dy = event.new_center.y - orig.y;
    host.replaceChildren();
    for (const p of sel) {
      if (p === draggedPath) continue;
      const g = geo.get(p);
      if (!g) continue;
      const sx = cam.a * (g.x + dx) + cam.e;
      const sy = cam.d * (g.y + dy) + cam.f;
      const w = Math.abs(cam.a) * g.w;
      const h = Math.abs(cam.a) * g.h;
      const ghost = document.createElement("div");
      ghost.style.cssText = `position:absolute;left:${sx - w / 2}px;top:${sy - h / 2}px;width:${w}px;height:${h}px;border:2px dashed rgba(77,159,255,.55);border-radius:3px`;
      host.appendChild(ghost);
    }
  };

  const handleEditEvent = (raw: unknown) => {
    const event = raw as SchematicEditEvent;
    if (event.edit_event_type !== "edit_schematic_component_location") return;
    if (event.in_progress) {
      updateGroupGhosts(event);
      return;
    }
    groupGhostsRef.current?.replaceChildren();
    // The LATEST hash, not the displayed view's — a red board's display is
    // pinned to the last good build, whose hash would bounce the move.
    if (!view || !latestHash || !event.schematic_component_id || !event.new_center) return;
    const draggedPath = view.id_map[event.schematic_component_id];
    if (!draggedPath) return;
    const centers = new Map<string, { x: number; y: number }>();
    for (const el of view.circuit_json) {
      if (el.type !== "schematic_component") continue;
      const id = el.schematic_component_id;
      const path = typeof id === "string" ? view.id_map[id] : undefined;
      const center = el.center as { x: number; y: number } | undefined;
      if (path && center) centers.set(path, center);
    }
    const moves: Record<string, MoveIn> = {};
    const old = centers.get(draggedPath);
    if (selectionRef.current.includes(draggedPath) && selectionRef.current.length > 1 && old) {
      const dx = event.new_center.x - old.x;
      const dy = event.new_center.y - old.y;
      for (const path of selectionRef.current) {
        const c = centers.get(path);
        if (c) moves[path] = { x: c.x + dx, y: c.y + dy };
      }
    } else {
      moves[draggedPath] = { x: event.new_center.x, y: event.new_center.y };
    }
    if (Object.keys(moves).length > 0) {
      onSavePositions(moves, latestHash);
    }
  };

  return (
    <div
      className={`dotgrid relative min-w-0 flex-1 overflow-hidden ${
        labelMode && !prompt ? "cursor-copy" : ""
      }`}
      ref={wrapRef}
      onClick={onContainerClick}
      onDoubleClick={onContainerDoubleClick}
    >
      {view && view.circuit_json.length > 0 ? (
        <SchematicViewer
          key={source ?? "no-board"}
          circuitJson={withEditCount(view.circuit_json)}
          editingEnabled
          defaultEditMode
          hideChrome
          cameraApiRef={cameraRef}
          onEditEvent={handleEditEvent}
          containerStyle={{
            width: "100%",
            height: "100%",
            opacity: dimmed ? 0.5 : 1,
            backgroundColor: "transparent",
          }}
          colorOverrides={{
            schematic: {
              background: "transparent",
              grid: "#e6e4dd",
              component_outline: "#232b3f",
              component_body: "#ffffff",
              reference: "#232b3f",
              value: "#6c7385",
              pin: "#6c7385",
              pin_name: "#6c7385",
              pin_number: "#9aa0ae",
              label_local: "#232b3f",
              label_global: "#c1783c",
              label_background: "rgba(251, 250, 247, 0.9)",
              net_name: "#6c7385",
              wire: "#c1783c",
              junction: "#c1783c",
            },
          }}
          css={highlightCss}
          onSchematicComponentClicked={({ schematicComponentId, event }) => {
            clickHandledRef.current = true;
            const path = view.id_map[schematicComponentId];
            if (path) applyClick(path, event.shiftKey);
          }}
        />
      ) : (
        <div className="canvas-empty" />
      )}

      <div ref={groupGhostsRef} className="pointer-events-none absolute inset-0" />
      <div
        ref={marqueeRef}
        className="pointer-events-none absolute z-10 border border-sky/70 bg-sky/10"
        style={{ display: "none" }}
      />

      {deleteConfirm && (
        <div
          role="dialog"
          aria-label="Delete components"
          className="absolute left-1/2 top-3 z-10 w-64 -translate-x-1/2 rounded-[14px] bg-white p-2.5 shadow-island ring-1 ring-ink/10"
        >
          <div className="mb-2 text-[12px] text-ink/85">
            Delete {deleteConfirm.paths.length} components? Unused nets they leave
            behind are removed too.
          </div>
          <div className="flex gap-1.5">
            <Button
              variant="quiet"
              tone="danger"
              size="sm"
              className="flex-1"
              disabled={deleteConfirm.busy}
              onClick={() => removeSelection(deleteConfirm.paths)}
            >
              Delete {deleteConfirm.paths.length}
            </Button>
            <Button
              variant="ghost"
              size="sm"
              className="flex-1"
              disabled={deleteConfirm.busy}
              onClick={() => setDeleteConfirm(null)}
            >
              Keep
            </Button>
          </div>
        </div>
      )}

      {view && view.circuit_json.length > 0 && (
        <GestureOverlay
          view={view}
          wrapRef={wrapRef}
          onPortClick={applyClick}
          labelArmed={labelMode !== null && !prompt}
          onPortLabel={(port, screen) =>
            setPrompt({ kind: "label", path: port.path, pin: port.pin, screen })
          }
          onWireConnect={(a, b, screen) => void wireConnect(a, b, false, screen)}
          onWireToNet={(a, net) => {
            const run = () =>
              void onAttachPin(a.path, a.pin, net, "Net").catch((err) =>
                showToast(err, {
                  retry: run,
                  intent: `Please connect ${a.path} pin ${a.pin} to the net ${net}`,
                }),
              );
            run();
          }}
        />
      )}

      {mergeConfirm && (
        <div
          role="dialog"
          aria-label="Merge nets"
          className="absolute z-10 w-64 rounded-[14px] bg-white p-2.5 shadow-island ring-1 ring-ink/10"
          style={(() => {
            const wrap = wrapRef.current?.getBoundingClientRect();
            const x = mergeConfirm.screen.x - (wrap?.left ?? 0);
            const y = mergeConfirm.screen.y - (wrap?.top ?? 0);
            return {
              left: Math.max(8, Math.min(x + 14, (wrap?.width ?? 600) - 264)),
              top: Math.max(8, Math.min(y + 14, (wrap?.height ?? 400) - 110)),
            };
          })()}
        >
          <div className="mb-2 text-[12px] text-ink/85">
            Merge <span className="font-mono font-bold">{mergeConfirm.from}</span> into{" "}
            <span className="font-mono font-bold">{mergeConfirm.into}</span>?{" "}
            {mergeConfirm.fromRefs} pin{mergeConfirm.fromRefs === 1 ? "" : "s"} move
            {mergeConfirm.fromRefs === 1 ? "s" : ""}.
          </div>
          <div className="flex gap-1.5">
            <Button
              variant="copper"
              size="sm"
              className="flex-1"
              disabled={mergeConfirm.busy}
              onClick={() => {
                setMergeConfirm({ ...mergeConfirm, busy: true });
                void wireConnect(mergeConfirm.a, mergeConfirm.b, true, mergeConfirm.screen);
              }}
            >
              {mergeConfirm.busy ? "Merging…" : "Merge nets"}
            </Button>
            <Button
              variant="ghost"
              size="sm"
              className="flex-1"
              disabled={mergeConfirm.busy}
              onClick={() => setMergeConfirm(null)}
            >
              Keep separate
            </Button>
          </div>
        </div>
      )}

      {labelMode && !prompt && (
        <div className="pointer-events-none absolute left-1/2 top-3 -translate-x-1/2 rounded-full bg-white px-3.5 py-[5px] text-[11px] font-medium text-ink/70 shadow-island">
          label <span className="font-bold">{labelMode.label}</span> · click a pin · Esc
          done
        </div>
      )}

      {wireHint && !labelMode && !placement && !prompt && !mergeConfirm && (
        <div className="pointer-events-none absolute left-1/2 top-3 -translate-x-1/2 rounded-full bg-white px-3.5 py-[5px] text-[11px] font-medium text-ink/70 shadow-island">
          drag pin to pin to wire
        </div>
      )}

      {prompt && view && (
        <InlinePrompt
          key={
            prompt.kind +
            (prompt.kind === "label"
              ? prompt.path + prompt.pin
              : prompt.kind === "rename"
                ? prompt.net
                : prompt.kind === "value"
                  ? prompt.path
                  : prompt.from)
          }
          title={
            prompt.kind === "label"
              ? "Attach a net to this pin"
              : prompt.kind === "rename"
                ? "Rename net"
                : prompt.kind === "value"
                  ? "Edit value"
                  : "Rename instance"
          }
          label={
            prompt.kind === "value"
              ? "Value"
              : prompt.kind === "iname"
                ? "Instance name"
                : "Net name"
          }
          initial={
            prompt.kind === "rename"
              ? prompt.net
              : prompt.kind === "value"
                ? prompt.current
                : prompt.kind === "iname"
                  ? prompt.from
                  : labelMode?.defaultName ||
                    `${
                      prompt.path
                        .split(".")
                        .filter((s) => s !== "root")
                        .slice(-2)[0] ?? "N"
                    }_${prompt.pin}`
          }
          verb={
            prompt.kind === "label"
              ? "Attach to"
              : prompt.kind === "value"
                ? "Set value to"
                : "Rename to"
          }
          busyVerb={
            prompt.kind === "label"
              ? "Attaching"
              : prompt.kind === "value"
                ? "Setting"
                : "Renaming"
          }
          screen={prompt.screen}
          wrapRef={wrapRef}
          onCommit={async (value) => {
            if (prompt.kind === "label") {
              await onAttachPin(prompt.path, prompt.pin, value, labelMode?.kind ?? "Net");
            } else if (prompt.kind === "rename") {
              await onRenameNet(prompt.net, value);
            } else if (prompt.kind === "value") {
              await onSetAttribute(prompt.path, "value", value);
            } else {
              await onRenameInstance(prompt.from, value);
              // Instance paths changed with the name; the selection is stale.
              onSelectRef.current([]);
            }
            // Label mode stays armed for the next pin (repeat labeling).
            setPrompt(null);
          }}
          onClose={() => setPrompt(null)}
        />
      )}

      <ProvisionalLayer
        parts={provisionals}
        wrapRef={wrapRef}
        onPinClick={(port, screen) =>
          setPrompt({ kind: "label", path: port.path, pin: port.pin, screen })
        }
        onMove={(moves) => {
          if (latestHash) onSavePositions(moves, latestHash);
        }}
        onMoved={onProvisionalMoved}
      />

      {view && placement && (
        <PlacementLayer
          key={placement.spec + placement.label}
          view={view}
          wrapRef={wrapRef}
          arm={placement}
          takenNames={provisionals.map((p) => p.name)}
          onCommit={onPlacementCommit}
          onFinish={onPlacementFinish}
        />
      )}

      {view && view.circuit_json.length > 0 && (
        <div className="absolute bottom-2.5 right-2.5 flex items-center gap-0.5 rounded-full bg-white px-1 py-0.5 shadow-island">
          {(
            [
              { label: "Zoom out (-)", icon: <IconMinus />, act: () => cameraRef.current?.zoom(1 / ZOOM_STEP) },
              { label: "Zoom to fit (F)", icon: <IconCornersOut />, act: () => cameraRef.current?.fit() },
              { label: "Zoom in (+)", icon: <IconPlus />, act: () => cameraRef.current?.zoom(ZOOM_STEP) },
            ] as const
          ).map(({ label, icon, act }) => (
            <button
              key={label}
              type="button"
              aria-label={label}
              title={label}
              className="flex h-6 w-6 items-center justify-center rounded-full text-ink/55 transition-colors hover:bg-ink/5 hover:text-ink"
              onClick={(e) => {
                e.stopPropagation();
                act();
              }}
            >
              {icon}
            </button>
          ))}
        </div>
      )}

      {dimmed && (
        <div className="pointer-events-none absolute left-1/2 top-3 -translate-x-1/2 rounded-full bg-white px-3.5 py-[5px] font-mono text-[10.5px] text-alert shadow-island">
          build failing — see Problems
        </div>
      )}

      {toast && (
        <div
          className="absolute bottom-10 left-1/2 flex max-w-[75%] -translate-x-1/2 items-center gap-2 rounded-full bg-white py-[4px] pl-3.5 pr-1.5 text-[11px] font-medium text-ink/80 shadow-island"
          title={toast.fe.detail}
        >
          <span className="truncate">{toast.fe.message}</span>
          {toast.fe.kind === "stale" && toast.retry && (
            <Button
              variant="copper"
              size="sm"
              className="!py-0.5 text-[10.5px]"
              onClick={() => {
                const retry = toast.retry;
                setToast(null);
                retry?.();
              }}
            >
              Try again
            </Button>
          )}
          {toast.fe.kind === "refusal" && toast.intent && (
            <Button
              variant="quiet"
              size="sm"
              className="!py-0.5 text-[10.5px]"
              onClick={() => {
                const t = toast;
                setToast(null);
                onAskAgent(
                  `${t.intent}. The canvas refused this edit with: "${t.fe.detail}" — please make the change in the source.`,
                );
              }}
            >
              Ask the agent
            </Button>
          )}
        </div>
      )}

      {selection.length > 0 && (
        <div
          className="pointer-events-none absolute bottom-2.5 left-2.5 max-w-[60%] truncate rounded-full bg-white px-3 py-1 font-mono text-[10.5px] shadow-island ring-1 ring-sky/40"
          title={selection.join("\n")}
        >
          <span className="font-bold text-sky">{selection.length} selected</span>
          <span className="text-ink/55"> · {selChipText}</span>
          {selGenerated && <span className="text-ink/40"> · generated, read-only</span>}
        </div>
      )}
    </div>
  );
}
