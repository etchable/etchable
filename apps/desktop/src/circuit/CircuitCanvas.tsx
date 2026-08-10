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
import {
  Button,
  IconCornersOut,
  IconCrosshair,
  IconCursor,
  IconHand,
  IconMinus,
  IconPlus,
  IconTag,
} from "@etchable/ui";
import { observeCamera, readCamera } from "./camera";
import { humanizeError, type FriendlyError } from "./errors";
import GestureOverlay from "./GestureOverlay";
import { SNAP, snap } from "./grid";
import { centersFromView, movesFromEdit } from "./moves";
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
/** Arrow-key nudge directions in grid steps. Schematic space is y-up, so Up
 * increases y. */
const NUDGES: Record<string, [number, number]> = {
  ArrowLeft: [-1, 0],
  ArrowRight: [1, 0],
  ArrowUp: [0, 1],
  ArrowDown: [0, -1],
};

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
  /** Detach one pin from its net — how a selected wire or net label is deleted. */
  onDisconnectPin: (path: string, pin: string) => Promise<void>;
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
// The last drawing we stamped, by content. Rebuilds are frequent — every agent
// edit, every watcher event, every save — and most of them don't move anything
// on the canvas. The viewer regenerates and swaps its whole <svg> whenever this
// key changes, which reads as a flash, so a build whose circuit_json is
// byte-identical reuses the previous array and the viewer never notices.
// (Deterministic emission is what makes the comparison sound: see
// crates/zen-build/src/circuit_json.rs.)
let lastDrawing: { json: string; elements: BuildView["circuit_json"] } | null = null;
function withEditCount(elements: BuildView["circuit_json"]): ViewerCircuitJson {
  if (!stamped.has(elements)) {
    const json = JSON.stringify(elements);
    if (lastDrawing && lastDrawing.json === json) {
      return lastDrawing.elements as unknown as ViewerCircuitJson;
    }
    stamped.add(elements);
    (elements as unknown as { editCount: number }).editCount = ++editSeq;
    lastDrawing = { json, elements };
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
    onDisconnectPin,
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
  const onDisconnectRef = useRef(onDisconnectPin);
  onDisconnectRef.current = onDisconnectPin;
  const onSaveRef = useRef(onSavePositions);
  onSaveRef.current = onSavePositions;

  // Open confirmations, mirrored into refs for the []-dep key handler.
  const mergeConfirmRef = useRef<unknown>(null);
  const deleteConfirmRef = useRef<unknown>(null);

  // Delete needs a confirmation when it cascades (>3 instances).
  const [deleteConfirm, setDeleteConfirm] = useState<{
    paths: string[];
    busy: boolean;
  } | null>(null);
  deleteConfirmRef.current = deleteConfirm;

  /** The component the pointer is over, from the overlay's hover tracking. */
  const hoveredRef = useRef<string | null>(null);

  /** `W` arms the wire tool: click a pin, click another. Stays armed for the
   * next wire (KiCad keeps its tool active) until Escape. */
  const [wireMode, setWireMode] = useState(false);
  const wireModeRef = useRef(wireMode);
  wireModeRef.current = wireMode;

  /** The hand tool: a sticky pan mode, for when holding space is awkward. */
  const [panMode, setPanMode] = useState(false);
  const panModeRef = useRef(panMode);
  panModeRef.current = panMode;

  /** Space held = pan mode, so a left-drag pans instead of rubber-banding. */
  const spaceHeldRef = useRef(false);
  const [panCursor, setPanCursor] = useState(false);
  useEffect(() => {
    const set = (held: boolean) => {
      if (spaceHeldRef.current === held) return;
      spaceHeldRef.current = held;
      setPanCursor(held);
    };
    const down = (e: KeyboardEvent) => {
      if (e.code !== "Space" && e.key !== " ") return;
      const el = document.activeElement;
      if (
        el instanceof HTMLElement &&
        (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.isContentEditable)
      ) {
        return;
      }
      e.preventDefault(); // space must not scroll or re-click a focused button
      set(true);
    };
    const up = (e: KeyboardEvent) => {
      if (e.code === "Space" || e.key === " ") set(false);
    };
    // Losing focus mid-hold would otherwise leave pan mode stuck on.
    const clear = () => set(false);
    window.addEventListener("keydown", down);
    window.addEventListener("keyup", up);
    window.addEventListener("blur", clear);
    return () => {
      window.removeEventListener("keydown", down);
      window.removeEventListener("keyup", up);
      window.removeEventListener("blur", clear);
    };
  }, []);

  /**
   * Which drags pan (patched into the viewer as `shouldPanOnDrag`): middle
   * button, or left button with space held. Plain left-drag is left alone for
   * marquee/move. Wheel events MUST pass — the underlying hook runs this same
   * predicate for zoom, so returning false here would kill wheel zoom.
   */
  const shouldPanOnDrag = (e: MouseEvent | TouchEvent | WheelEvent) => {
    if (typeof WheelEvent !== "undefined" && e instanceof WheelEvent) return true;
    if (typeof MouseEvent !== "undefined" && e instanceof MouseEvent) {
      const middle = e.button === 1 || (e.buttons & 4) !== 0;
      return middle || spaceHeldRef.current || panModeRef.current;
    }
    return true; // touch panning is unchanged
  };

  /**
   * What a keyboard verb acts on. KiCad acts on the item under the cursor when
   * nothing is selected — that hover-first reflex is most of why its editing
   * feels direct — so the selection wins when it exists and the hovered
   * component stands in for it when it doesn't.
   */
  const actionTargets = () => {
    if (selectionRef.current.length > 0) return selectionRef.current;
    return hoveredRef.current ? [hoveredRef.current] : [];
  };

  /** Deletable instance paths among the action targets (nets filtered out). */
  const deletableSelection = () => {
    const v = viewRef.current;
    if (!v?.editability) return [];
    return actionTargets().filter((p) => v.editability?.instances[p]);
  };

  /**
   * Deleting a selected wire or net label means detaching pins — a net isn't a
   * thing in the source, it's the connection between pins. Unambiguous only
   * when the net joins exactly two pins (the wire you clicked IS that
   * connection); past that, which end to drop is the user's call, so hand it to
   * the agent rather than guess.
   */
  const deleteNets = (nets: string[]) => {
    const v = viewRef.current;
    const sch = v?.schematic;
    if (!sch) return;
    for (const net of nets) {
      const ports = sch.nets?.[net]?.ports ?? [];
      if (ports.length !== 2) {
        showToastErrRef.current(
          `${net} joins ${ports.length} pins — deleting it means choosing which to detach.`,
          { intent: `Please disconnect ${net}` },
        );
        continue;
      }
      void Promise.all(
        ports.map((p) => onDisconnectRef.current(p.component, p.pin)),
      )
        .then(() => {
          onSelectRef.current([]);
          showToastRef.current(
            `Disconnected ${ports.map((p) => `${p.component.split(".").pop()}.${p.pin}`).join(" and ")} from ${net}.`,
          );
        })
        .catch((err) => showToastErrRef.current(err));
    }
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
      if (path && c && actionTargets().includes(path)) {
        moves[path] = { x: c.x, y: c.y, rotate_by: 90 };
      }
    }
    if (Object.keys(moves).length > 0) onSaveRef.current(moves, hash);
  };

  /**
   * Arrow-key nudge, one grid step per press. Snapped, so nudging something
   * that came off a derived layout pulls it onto the grid on the first press
   * instead of carrying an arbitrary offset around forever.
   */
  const nudgeSelection = (dx: number, dy: number) => {
    const v = viewRef.current;
    const hash = latestHashRef.current;
    if (!v || !hash) return;
    const targets = actionTargets();
    if (targets.length === 0) return;
    const centers = centersFromView(v);
    const moves: Record<string, MoveIn> = {};
    for (const path of targets) {
      const c = centers.get(path);
      if (c) moves[path] = { x: snap(c.x + dx), y: snap(c.y + dy) };
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
  const [copiedToast, setCopiedToast] = useState(false);
  const copiedTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  /** Copy the toast's full text (detail, which carries the raw error). */
  const copyToast = () => {
    const t = toastRef.current;
    if (!t) return;
    const text = t.fe.detail ?? t.fe.message;
    void navigator.clipboard
      .writeText(text)
      .then(() => {
        setCopiedToast(true);
        if (copiedTimer.current) clearTimeout(copiedTimer.current);
        // Hold the toast open long enough to read the confirmation.
        if (toastTimer.current) clearTimeout(toastTimer.current);
        toastTimer.current = setTimeout(() => setToast(null), 2500);
        copiedTimer.current = setTimeout(() => setCopiedToast(false), 1500);
      })
      .catch(() => {});
  };
  const toastRef = useRef(toast);
  toastRef.current = toast;
  const armToast = (t: NonNullable<typeof toast>, ms: number) => {
    setToast(t);
    setCopiedToast(false);
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
  mergeConfirmRef.current = mergeConfirm;

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

  // Marquee select: plain left-drag on empty canvas draws a rubber band and
  // selects the components whose centers fall inside — the KiCad (and Figma)
  // model, where dragging nothing means "select an area" and panning has its
  // own buttons (middle-drag, or space held). Shift makes it additive. The
  // capture-phase mousedown keeps the viewer from acting on the same press.
  const marqueeRef = useRef<HTMLDivElement>(null);
  const additiveRef = useRef(false);
  useEffect(() => {
    const wrap = wrapRef.current;
    if (!wrap) return;
    let start: { x: number; y: number } | null = null;
    // A pan is a camera gesture, not a selection one: remember that the press
    // became a drag so the click it produces doesn't clear the selection.
    let panFrom: { x: number; y: number } | null = null;
    const panDown = (e: MouseEvent) => {
      if (e.button === 1 || (e.button === 0 && spaceHeldRef.current)) {
        panFrom = { x: e.clientX, y: e.clientY };
      }
    };
    const panUp = (e: MouseEvent) => {
      if (!panFrom) return;
      const moved = Math.hypot(e.clientX - panFrom.x, e.clientY - panFrom.y) >= 4;
      panFrom = null;
      if (moved) clickHandledRef.current = true;
    };
    const down = (e: MouseEvent) => {
      if (e.button !== 0) return; // middle-drag pans
      if (spaceHeldRef.current) return; // space-drag pans
      if (panModeRef.current) return; // the hand tool owns the canvas
      if (wireModeRef.current) return; // the wire tool owns the canvas
      const target = e.target as Element;
      if (target.closest?.(".pin-hit")) return; // a wire gesture starts here
      // Pressing a symbol (or a net label) is that thing's own gesture: the
      // viewer moves it, a click selects it. Only empty canvas rubber-bands —
      // except with shift, which is explicitly "add an area to the selection".
      if (
        !e.shiftKey &&
        target.closest?.("[data-schematic-component-id], [data-schematic-net-label-id]")
      ) {
        return;
      }
      e.preventDefault();
      e.stopPropagation();
      additiveRef.current = e.shiftKey;
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
      if (additiveRef.current) {
        const merged = [...new Set([...selectionRef.current, ...hit])];
        if (merged.length !== selectionRef.current.length) onSelectRef.current(merged);
      } else {
        // An empty band clears, the way clicking empty canvas does.
        onSelectRef.current(hit);
      }
    };
    wrap.addEventListener("mousedown", down, true);
    wrap.addEventListener("mousedown", panDown);
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
    window.addEventListener("mouseup", panUp);
    return () => {
      wrap.removeEventListener("mousedown", down, true);
      wrap.removeEventListener("mousedown", panDown);
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
      window.removeEventListener("mouseup", panUp);
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
      // Any open confirmation takes Esc first — a card with no keyboard exit is
      // a trap, especially when it appeared because something failed.
      if (e.key === "Escape" && mergeConfirmRef.current) {
        setMergeConfirm(null);
        return;
      }
      if (e.key === "Escape" && deleteConfirmRef.current) {
        setDeleteConfirm(null);
        return;
      }
      // Esc disarms the wire tool before it touches the selection.
      if (e.key === "Escape" && wireModeRef.current) {
        setWireMode(false);
        return;
      }
      if (e.key === "w" || e.key === "W") {
        setPanMode(false);
        setWireMode((on) => !on);
        return;
      }
      if (e.key === "v" || e.key === "V") {
        setWireMode(false);
        setPanMode(false);
        return;
      }
      if (e.key === "h" || e.key === "H") {
        setWireMode(false);
        setPanMode((on) => !on);
        return;
      }
      if (e.key === "Escape" && panModeRef.current) {
        setPanMode(false);
        return;
      }
      // Esc ends an armed label tool before it clears the selection.
      if (e.key === "Escape" && labelModeRef.current) {
        onLabelFinishRef.current();
        return;
      }
      if (e.key === "Escape" && selectionRef.current.length > 0) {
        onSelectRef.current([]);
      } else if (e.key === "Delete" || e.key === "Backspace") {
        const paths = deletableSelection();
        // Wires and net labels select as NETS, which the instance-only filter
        // above drops — that used to make Delete a silent no-op on them.
        const v = viewRef.current;
        const nets = actionTargets().filter((t) => v?.editability?.nets[t]);
        if (nets.length > 0) {
          e.preventDefault();
          deleteNets(nets);
        }
        if (paths.length === 0) return;
        e.preventDefault();
        if (paths.length > 3) setDeleteConfirm({ paths, busy: false });
        else removeSelection(paths);
      } else if ((e.key === "r" || e.key === "R") && actionTargets().length > 0) {
        rotateSelection();
      } else if (e.key === "+" || e.key === "=") {
        cameraRef.current?.zoom(ZOOM_STEP);
      } else if (e.key === "-") {
        cameraRef.current?.zoom(1 / ZOOM_STEP);
      } else if (e.key === "f" || e.key === "0" || e.key === "Home") {
        // Home is the KiCad reflex for zoom-to-fit; f/0 stay as they were.
        cameraRef.current?.fit();
      } else if (e.key === "e" || e.key === "E") {
        // Edit the one thing being acted on; ambiguous for a multi-selection.
        const targets = actionTargets();
        if (targets.length !== 1) return;
        const at = screenPointOf(targets[0]);
        if (!at) return;
        e.preventDefault();
        openEditFor(targets[0], at);
      } else if (NUDGES[e.key]) {
        // One grid step per press, on the selection or the hovered part.
        const [dx, dy] = NUDGES[e.key];
        if (actionTargets().length > 0) {
          e.preventDefault();
          nudgeSelection(dx * SNAP, dy * SNAP);
        }
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
          `g[data-schematic-component-id="${cssAttr(compId)}"] { filter: drop-shadow(0 0 1px ${ACCENT}) drop-shadow(0 0 6px ${ACCENT}); }`,
        );
      }
      for (const labelId of reverse.labelIdsByNet.get(target) ?? []) {
        rules.push(
          `[data-schematic-net-label-id="${cssAttr(labelId)}"] { filter: drop-shadow(0 0 1px ${ACCENT}) drop-shadow(0 0 6px ${ACCENT}); }`,
        );
      }
      for (const traceId of reverse.traceIdsByNet.get(target) ?? []) {
        rules.push(
          `g[data-schematic-trace-id="${cssAttr(traceId)}"] { filter: drop-shadow(0 0 1px ${ACCENT}) drop-shadow(0 0 6px ${ACCENT}); }`,
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
  /**
   * Open the inline editor for a component: its value if it has one, else its
   * name. Shared by double-click and the `E`/`V` keys — in KiCad those keys
   * are the reflex for "edit this thing", and having both reach the same
   * editor is what keeps the two habits from diverging.
   */
  const openEditFor = (path: string, screen: { x: number; y: number }) => {
    const v = viewRef.current;
    if (!v) return;
    const edit = v.editability?.instances[path];
    if (edit && !edit.editable && !edit.anchor) {
      showToastErrRef.current(
        edit.reason ?? `${path} is generated and can't be edited directly`,
        { intent: `Please edit ${path} for me` },
      );
      return;
    }
    const value = v.schematic?.instances[path]?.attributes?.value;
    if (value !== undefined) {
      setPrompt({ kind: "value", path, current: String(value), screen });
    } else {
      const anchor = edit && !edit.editable ? edit.anchor : path;
      const from = (anchor ?? path).split(".").pop();
      if (!from) return;
      setPrompt({ kind: "iname", from, screen });
    }
  };

  /** Where a component currently sits on screen, for keyboard-opened editors
   * that need an anchor the way a click gives one. */
  const screenPointOf = (path: string): { x: number; y: number } | null => {
    const v = viewRef.current;
    const wrap = wrapRef.current;
    if (!v || !wrap) return null;
    const center = centersFromView(v).get(path);
    const cam = readCamera(wrap);
    if (!center || !cam) return null;
    const rect = wrap.getBoundingClientRect();
    return {
      x: rect.left + cam.a * center.x + cam.e,
      y: rect.top + cam.d * center.y + cam.f,
    };
  };

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
    openEditFor(path, { x: e.clientX, y: e.clientY });
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
    // A drag ends with a click, and the container's click handler treats a
    // plain click as "clear the selection" — which threw away a multi-select
    // the moment you let go of a group drag. Dragging is not a selection
    // gesture, so swallow that click.
    clickHandledRef.current = true;
    // The LATEST hash, not the displayed view's — a red board's display is
    // pinned to the last good build, whose hash would bounce the move.
    if (!view || !latestHash || !event.schematic_component_id || !event.new_center) return;
    const draggedPath = view.id_map[event.schematic_component_id];
    if (!draggedPath) return;
    const moves = movesFromEdit({
      draggedPath,
      newCenter: event.new_center,
      centers: centersFromView(view),
      selection: selectionRef.current,
    });
    if (Object.keys(moves).length > 0) {
      onSavePositions(moves, latestHash);
    }
  };

  return (
    <div
      className={`dotgrid relative min-w-0 flex-1 overflow-hidden ${
        labelMode && !prompt ? "cursor-copy" : ""
      } ${panCursor || panMode ? "pan-mode" : ""} ${wireMode ? "wire-mode" : ""} ${
        !panCursor && !panMode && !wireMode && !labelMode && !placement ? "select-mode" : ""
      }`}
      ref={wrapRef}
      tabIndex={-1}
      onMouseDownCapture={(e) => {
        // Clicking a plain <div> doesn't move focus, so the chat textarea keeps
        // it and isTyping() silences W/V/H/R/Delete for the rest of the
        // session. Hand focus to the canvas — unless the press is inside a
        // field that legitimately wants it (the inline prompt).
        const target = e.target as HTMLElement | null;
        if (
          target &&
          (target.tagName === "INPUT" ||
            target.tagName === "TEXTAREA" ||
            target.isContentEditable)
        ) {
          return;
        }
        const active = document.activeElement;
        if (
          active instanceof HTMLElement &&
          (active.tagName === "INPUT" ||
            active.tagName === "TEXTAREA" ||
            active.isContentEditable)
        ) {
          active.blur();
        }
        wrapRef.current?.focus({ preventScroll: true });
      }}
      onClick={onContainerClick}
      onDoubleClick={onContainerDoubleClick}
    >
      {view && view.circuit_json.length > 0 ? (
        <SchematicViewer
          key={source ?? "no-board"}
          circuitJson={withEditCount(view.circuit_json)}
          editingEnabled
          shouldPanOnDrag={shouldPanOnDrag}
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
          selection={selection}
          wireArmed={wireMode}
          onHoverComponent={(path) => {
            hoveredRef.current = path;
          }}
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

      {/* Mode selector. The keyboard is the fast path (W, Esc) but a mode you
          can't see is a mode you don't use, so it also reads as the current
          state: exactly one segment is active at a time. Label mode is armed
          from the palette (it carries which net to attach), so it appears here
          only while active — as an indicator you can click to leave. */}
      <div className="absolute left-3 top-3 flex overflow-hidden rounded-full bg-white p-[3px] shadow-island">
        {[
          {
            key: "pan",
            label: "Pan",
            hotkey: "H",
            hint: "drag to move the canvas (or hold space / middle-drag)",
            Icon: IconHand,
            active: panMode,
            onPick: () => {
              setWireMode(false);
              if (labelMode) onLabelFinish();
              if (placement) onPlacementFinish();
              setPanMode(true);
            },
          },
          {
            key: "select",
            label: "Select",
            hotkey: "V",
            hint: "click to select · drag to rubber-band · Esc also returns here",
            Icon: IconCursor,
            active: !wireMode && !panMode && !labelMode && !placement,
            onPick: () => {
              setWireMode(false);
              setPanMode(false);
              if (labelMode) onLabelFinish();
              if (placement) onPlacementFinish();
            },
          },
          {
            key: "wire",
            label: "Wire",
            hotkey: "W",
            hint: "click two pins",
            Icon: IconCrosshair,
            active: wireMode,
            onPick: () => {
              setPanMode(false);
              if (labelMode) onLabelFinish();
              if (placement) onPlacementFinish();
              setWireMode(true);
            },
          },
          ...(labelMode
            ? [
                {
                  key: "label",
                  label: labelMode.label,
                  hotkey: "Esc",
                  hint: "click a pin to attach; Esc to finish",
                  Icon: IconTag,
                  active: true,
                  onPick: () => onLabelFinish(),
                },
              ]
            : []),
        ].map(({ key, label, hotkey, hint, Icon, active, onPick }) => (
          <button
            key={key}
            type="button"
            title={`${label} (${hotkey}) — ${hint}`}
            aria-keyshortcuts={hotkey}
            aria-pressed={active}
            onClick={onPick}
            className={`flex items-center gap-1.5 rounded-full px-2.5 py-[3px] text-[11px] font-medium transition-colors ${
              active ? "bg-ink text-white" : "text-ink/60 hover:bg-ink/5"
            }`}
          >
            <Icon size={13} />
            {label}
            {/* The shortcut rides along visibly — a tooltip nobody hovers is
                not discoverability. */}
            <kbd
              className={`rounded border px-1 font-mono text-[9px] leading-[14px] ${
                active ? "border-white/25 text-white/70" : "border-ink/15 text-ink/40"
              }`}
            >
              {hotkey}
            </kbd>
          </button>
        ))}
      </div>

      {wireMode && !prompt && (
        <div className="pointer-events-none absolute left-1/2 top-3 -translate-x-1/2 rounded-full bg-white px-3.5 py-[5px] text-[11px] font-medium text-ink/70 shadow-island">
          wire · click two pins · Esc done
        </div>
      )}

      {wireHint && !wireMode && !labelMode && !placement && !prompt && !mergeConfirm && (
        <div className="pointer-events-none absolute left-1/2 top-3 -translate-x-1/2 rounded-full bg-white px-3.5 py-[5px] text-[11px] font-medium text-ink/70 shadow-island">
          press <span className="font-bold">W</span> to wire
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

      {/* Selection + diagnostic highlighting. Deliberately NOT the viewer's
          `css` prop: that string is baked into the generated SVG, and the
          viewer's generation memo does not list `css` as a dependency, so
          highlights simply never appeared unless some other change happened to
          regenerate the SVG at the same moment. A plain stylesheet applies to
          the SVG the viewer already rendered — instant, and it never triggers a
          regeneration (which is also what keeps clicking around from flashing
          the board). */}
      <style>{highlightCss}</style>

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
          className="absolute bottom-10 left-1/2 flex max-w-[75%] -translate-x-1/2 cursor-copy items-center gap-2 rounded-full bg-white py-[4px] pl-3.5 pr-1.5 text-[11px] font-medium text-ink/80 shadow-island"
          title={`${toast.fe.detail ?? toast.fe.message}\n\nClick to copy`}
          role="button"
          tabIndex={0}
          onClick={() => copyToast()}
          onKeyDown={(e) => {
            if (e.key === "Enter" || e.key === " ") {
              e.preventDefault();
              copyToast();
            }
          }}
        >
          {/* Errors are the thing you most want to paste somewhere — into the
              chat, an issue, a search. The whole pill is the copy affordance. */}
          <span className="truncate">{copiedToast ? "Copied to clipboard" : toast.fe.message}</span>
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
