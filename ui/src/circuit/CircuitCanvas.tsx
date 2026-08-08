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
import type { BuildView, Diag } from "../types";

// The viewer bundles its own circuit-json types (a different version than
// our pin), so target its actual prop type rather than either package's.
type ViewerCircuitJson = ComponentProps<typeof SchematicViewer>["circuitJson"];

const ACCENT = "#7aa2ff";
const ERROR = "#f87171";
const WARNING = "#fbbf24";

type CircuitCanvasProps = {
  view: BuildView | null;
  source: string | null;
  dimmed: boolean;
  diagnostics: Diag[];
  selection: string[];
  onSelectionChange: (paths: string[]) => void;
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
  const { view, source, dimmed, diagnostics, selection, onSelectionChange } = props;

  const selectionRef = useRef(selection);
  selectionRef.current = selection;
  const onSelectRef = useRef(onSelectionChange);
  onSelectRef.current = onSelectionChange;
  // Set when a viewer callback handled the click, so the container's
  // background handler (which the same click bubbles into) doesn't clear it.
  const clickHandledRef = useRef(false);

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
  // net name -> net label ids.
  const reverse = useMemo(() => {
    const componentIdByPath = new Map<string, string>();
    const labelIdsByNet = new Map<string, string[]>();
    if (view) {
      for (const [id, target] of Object.entries(view.id_map)) {
        if (id.startsWith("sch:")) componentIdByPath.set(target, id);
        else if (id.startsWith("netlabel:")) {
          const list = labelIdsByNet.get(target);
          if (list) list.push(id);
          else labelIdsByNet.set(target, [id]);
        }
      }
    }
    return { componentIdByPath, labelIdsByNet };
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
    }
    return rules.join("\n");
  }, [selection, reverse, diagSeverityByPath]);

  // Delegated clicks: net labels (no viewer callback exists) and background
  // (clear selection). Component clicks arrive via the viewer callback and
  // set clickHandledRef before bubbling here.
  const onContainerClick = (e: React.MouseEvent<HTMLDivElement>) => {
    if (clickHandledRef.current) {
      clickHandledRef.current = false;
      return;
    }
    if (!view) return;
    const el = (e.target as Element).closest?.("[data-schematic-net-label-id]");
    if (el) {
      const id = el.getAttribute("data-schematic-net-label-id");
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

  return (
    <div className="canvas-wrap" onClick={onContainerClick}>
      {view && view.circuit_json.length > 0 ? (
        <SchematicViewer
          key={source ?? "no-board"}
          circuitJson={withEditCount(view.circuit_json)}
          containerStyle={{
            width: "100%",
            height: "100%",
            opacity: dimmed ? 0.5 : 1,
            backgroundColor: "transparent",
          }}
          colorOverrides={{
            schematic: {
              background: "#0e1116",
              grid: "#1a212c",
              component_outline: "#c7d0dc",
              component_body: "#151a22",
              reference: "#c7d0dc",
              value: "#8b95a3",
              pin: "#8b95a3",
              pin_name: "#8b95a3",
              pin_number: "#5a6472",
              label_local: "#e8eaf0",
              label_global: "#7aa2ff",
              label_background: "rgba(21, 26, 34, 0.85)",
              net_name: "#8b95a3",
              wire: "#5c9866",
              junction: "#5c9866",
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

      {dimmed && <div className="canvas-toast">build failing — see Problems</div>}

      {selection.length > 0 && (
        <div className="sel-chip" title={selection.join("\n")}>
          <span className="sel-chip-count">{selection.length} selected</span>
          <span className="sel-chip-paths"> · {selChipText}</span>
        </div>
      )}
    </div>
  );
}
