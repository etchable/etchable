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

import { useEffect, useMemo, useRef } from "react";
import type { ComponentProps } from "react";
import { SchematicViewer } from "@tscircuit/schematic-viewer";
import type { BuildView, Diag, PositionIn } from "../types";

// The viewer bundles its own circuit-json types (a different version than
// our pin), so target its actual prop type rather than either package's.
type ViewerCircuitJson = ComponentProps<typeof SchematicViewer>["circuitJson"];

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
  onSavePositions: (positions: Record<string, PositionIn>, baseHash: string) => void;
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
  const { view, source, dimmed, diagnostics, selection, onSelectionChange, onSavePositions } =
    props;

  const selectionRef = useRef(selection);
  selectionRef.current = selection;
  const onSelectRef = useRef(onSelectionChange);
  onSelectRef.current = onSelectionChange;
  // Set when a viewer callback handled the click, so the container's
  // background handler (which the same click bubbles into) doesn't clear it.
  const clickHandledRef = useRef(false);
  const wrapRef = useRef<HTMLDivElement>(null);

  // The paper dot grid lives on the container as a CSS background; the
  // viewer stamps its real->svg fit on the SVG as
  // data-real-to-screen-transform, and applies the user's pan/zoom as a CSS
  // matrix on the div wrapping that SVG. The full real->screen mapping is
  // the composition of the two; mirror it onto the background so the grid
  // pans and zooms with the board (adapting the cell by powers of 5 to keep
  // a sane density), instead of sitting behind it as static wallpaper.
  //
  // Camera stability across refits (container resizes, rebuild echoes) is
  // NOT handled here anymore: the vendored viewer patch
  // (patches/@tscircuit__schematic-viewer.patch) folds every fit change into
  // the viewer's own pan/zoom matrix, so the schematic stays locked in place
  // and this observer only has to keep the grid glued to it.
  // Direct style writes, not state — pan/zoom emits a mutation per frame.
  useEffect(() => {
    const wrap = wrapRef.current;
    if (!wrap) return;
    const parse = (raw: string | null | undefined) => {
      const m = raw?.match(/matrix\(([^)]*)\)/);
      if (!m) return null;
      const [a, b, c, d, e, f] = m[1].split(/[,\s]+/).map(Number);
      if (![a, b, c, d, e, f].every(Number.isFinite)) return null;
      return { a, d, e, f };
    };
    const sync = () => {
      const svg = wrap.querySelector("[data-real-to-screen-transform]");
      const fit = parse(svg?.getAttribute("data-real-to-screen-transform"));
      if (!fit || fit.a === 0) return;
      // The user pan/zoom matrix lives on an ancestor between the svg and
      // the container; identity when absent (fresh mount, no interaction).
      let user = { a: 1, d: 1, e: 0, f: 0 };
      for (let el = svg?.parentElement; el && el !== wrap; el = el.parentElement) {
        const t = el instanceof HTMLElement ? parse(el.style.transform) : null;
        if (t) {
          user = t;
          break;
        }
      }
      const scale = Math.abs(user.a * fit.a); // px per schematic unit
      const ex = user.a * fit.e + user.e;
      const fy = user.d * fit.f + user.f;
      if (!Number.isFinite(scale) || scale === 0) return;

      let cell = scale;
      while (cell < 18) cell *= 5;
      while (cell > 220) cell /= 5;
      wrap.style.backgroundSize = `${cell}px ${cell}px`;
      wrap.style.backgroundPosition = `${ex}px ${fy}px`;
    };
    sync();
    const observer = new MutationObserver(sync);
    observer.observe(wrap, {
      subtree: true,
      childList: true,
      attributes: true,
      attributeFilter: ["data-real-to-screen-transform", "style"],
    });
    return () => observer.disconnect();
  }, [source]);

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

  // Escape clears the selection (unless typing in an input).
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
      if (e.key === "Escape" && !isTyping() && selectionRef.current.length > 0) {
        onSelectRef.current([]);
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

  const selChipText = useMemo(() => {
    if (selection.length === 0) return "";
    const joined = selection.join(", ");
    return joined.length > 64 ? joined.slice(0, 64) + "…" : joined;
  }, [selection]);

  // Drag-to-move: the viewer fires one event per drag (on mouseup) with the
  // new center in schematic coordinates. Persist SAVE-ALL — every component's
  // current center, the dragged one overridden — because the layout's
  // authored-positions rule is all-or-nothing. Schematic -> authored space:
  // x*25.4, y negated (verified against the emitter's point()).
  const handleEditEvent = (raw: unknown) => {
    const event = raw as SchematicEditEvent;
    if (event.edit_event_type !== "edit_schematic_component_location") return;
    if (event.in_progress) return;
    if (!view?.source_hash || !event.schematic_component_id || !event.new_center) return;
    const draggedPath = view.id_map[event.schematic_component_id];
    if (!draggedPath) return;
    const positions: Record<string, PositionIn> = {};
    for (const el of view.circuit_json) {
      if (el.type !== "schematic_component") continue;
      const id = el.schematic_component_id;
      const path = typeof id === "string" ? view.id_map[id] : undefined;
      const center = el.center as { x: number; y: number } | undefined;
      if (!path || !center) continue;
      const c = path === draggedPath ? event.new_center : center;
      const authored = view.schematic?.instances[path]?.position;
      positions[path] = {
        x: 25.4 * c.x,
        y: -25.4 * c.y,
        rotation: authored?.rotation ?? 0,
        mirror: authored?.mirror ?? null,
      };
    }
    if (Object.keys(positions).length > 0) {
      onSavePositions(positions, view.source_hash);
    }
  };

  return (
    <div
      className="dotgrid relative min-w-0 flex-1 overflow-hidden"
      ref={wrapRef}
      onClick={onContainerClick}
    >
      {view && view.circuit_json.length > 0 ? (
        <SchematicViewer
          key={source ?? "no-board"}
          circuitJson={withEditCount(view.circuit_json)}
          editingEnabled
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

      {dimmed && (
        <div className="pointer-events-none absolute left-1/2 top-3 -translate-x-1/2 rounded-full bg-white px-3.5 py-[5px] font-mono text-[10.5px] text-alert shadow-island">
          build failing — see Problems
        </div>
      )}

      {selection.length > 0 && (
        <div
          className="pointer-events-none absolute bottom-2.5 left-2.5 max-w-[60%] truncate rounded-full bg-white px-3 py-1 font-mono text-[10.5px] shadow-island ring-1 ring-sky/40"
          title={selection.join("\n")}
        >
          <span className="font-bold text-sky">{selection.length} selected</span>
          <span className="text-ink/55"> · {selChipText}</span>
        </div>
      )}
    </div>
  );
}
