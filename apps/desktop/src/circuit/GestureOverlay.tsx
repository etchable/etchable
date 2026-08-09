// The gesture overlay (decision 0009 §7): pin targets, hover affordances,
// and the drag-to-wire gesture, rendered above the viewer and glued to its
// camera. Reworked per the UX review: pins show a faint RESTING dot while
// the pointer is over their component (wiring is discoverable, not
// secret), hit targets are larger than the visible ring, tooltips carry
// the editability reason, and wire drops report their screen point so
// confirmations can anchor at the gesture.

import { useEffect, useMemo, useRef } from "react";
import { observeCamera } from "./camera";
import type { BuildView } from "../types";

/** Hit target and visible ring, in constant screen px. */
const HIT_RADIUS_PX = 10;
const VIS_RADIUS_PX = 6;
const DRAG_THRESHOLD_PX = 5;

type PortTarget = {
  id: string;
  x: number;
  y: number;
  pin: string;
  /** Owning component's instance path (via id_map — ids are never parsed). */
  path: string;
  /** No structured edit can reach this pin, even through an anchor. */
  generated: boolean;
  /** Editability reason, for the tooltip. */
  reason: string | null;
};

type GestureOverlayProps = {
  view: BuildView;
  wrapRef: React.RefObject<HTMLDivElement | null>;
  onPortClick: (path: string, shiftKey: boolean) => void;
  /** When set, pin clicks label instead of selecting (phase 2). */
  labelArmed?: boolean;
  onPortLabel?: (
    port: { path: string; pin: string },
    screen: { x: number; y: number },
  ) => void;
  /** Drag pin→pin (phase 3): wire the two together. */
  onWireConnect?: (
    a: { path: string; pin: string },
    b: { path: string; pin: string },
    screen: { x: number; y: number },
  ) => void;
  /** Drag pin→trace/net-label: attach the pin to that net (junction). */
  onWireToNet?: (a: { path: string; pin: string }, net: string) => void;
};

export default function GestureOverlay(props: GestureOverlayProps) {
  const { view, wrapRef, onPortClick, labelArmed, onPortLabel, onWireConnect, onWireToNet } =
    props;
  const gRef = useRef<SVGGElement>(null);
  const lineRef = useRef<SVGLineElement>(null);
  // A wire drag in progress: set on pin mousedown, resolved on mouseup.
  const dragRef = useRef<{
    path: string;
    pin: string;
    startX: number;
    startY: number;
    active: boolean;
  } | null>(null);
  // Swallow the click that follows a completed drag.
  const suppressClickRef = useRef(false);

  useEffect(() => {
    const move = (e: MouseEvent) => {
      const drag = dragRef.current;
      const wrap = wrapRef.current;
      const line = lineRef.current;
      if (!drag || !wrap || !line) return;
      if (
        !drag.active &&
        Math.hypot(e.clientX - drag.startX, e.clientY - drag.startY) < DRAG_THRESHOLD_PX
      ) {
        return;
      }
      drag.active = true;
      const rect = wrap.getBoundingClientRect();
      line.style.display = "block";
      line.setAttribute("x1", String(drag.startX - rect.left));
      line.setAttribute("y1", String(drag.startY - rect.top));
      line.setAttribute("x2", String(e.clientX - rect.left));
      line.setAttribute("y2", String(e.clientY - rect.top));
    };
    const up = (e: MouseEvent) => {
      const drag = dragRef.current;
      dragRef.current = null;
      const line = lineRef.current;
      if (line) line.style.display = "none";
      if (!drag?.active) return;
      suppressClickRef.current = true;
      // What's under the drop? Another pin wires pin-to-pin; a routed
      // trace or net label attaches to that net (a junction).
      const el = document.elementFromPoint(e.clientX, e.clientY);
      const pinEl = el?.closest?.(".pin-hit") as SVGElement | null;
      if (pinEl) {
        const path = pinEl.getAttribute("data-port-path");
        const pin = pinEl.getAttribute("data-port-pin");
        if (path && pin && (path !== drag.path || pin !== drag.pin)) {
          onWireConnect?.(
            { path: drag.path, pin: drag.pin },
            { path, pin },
            { x: e.clientX, y: e.clientY },
          );
        }
        return;
      }
      for (const attr of ["data-schematic-trace-id", "data-schematic-net-label-id"]) {
        const hit = el?.closest?.(`[${attr}]`);
        const id = hit?.getAttribute(attr);
        const net = id ? view.id_map[id] : undefined;
        if (net) {
          onWireToNet?.({ path: drag.path, pin: drag.pin }, net);
          return;
        }
      }
    };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
    return () => {
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
    };
  }, [view, wrapRef, onWireConnect, onWireToNet]);

  const ports = useMemo<PortTarget[]>(() => {
    // Pin names live on source_port elements; schematic_port carries the
    // geometry and links back via source_port_id.
    const nameBySourcePort = new Map<string, string>();
    for (const el of view.circuit_json) {
      if (el.type !== "source_port") continue;
      const id = el.source_port_id;
      const name = el.name;
      if (typeof id === "string" && typeof name === "string") {
        nameBySourcePort.set(id, name);
      }
    }
    const out: PortTarget[] = [];
    for (const el of view.circuit_json) {
      if (el.type !== "schematic_port") continue;
      const id = el.schematic_port_id;
      const center = el.center as { x: number; y: number } | undefined;
      if (typeof id !== "string" || !center) continue;
      const path = view.id_map[id];
      if (!path) continue;
      const srcId = el.source_port_id;
      const edit = view.editability?.instances[path];
      const generated = edit ? !edit.editable && !edit.anchor : false;
      out.push({
        id,
        x: center.x,
        y: center.y,
        pin: (typeof srcId === "string" && nameBySourcePort.get(srcId)) || "",
        path,
        generated,
        reason: generated ? (edit?.reason ?? null) : null,
      });
    }
    return out;
  }, [view]);

  // Glue the target layer to the viewer camera: one transform on the group,
  // and per-frame radii so hit targets and rings keep constant screen size.
  useEffect(() => {
    const wrap = wrapRef.current;
    const g = gRef.current;
    if (!wrap || !g || ports.length === 0) return;
    return observeCamera(wrap, (cam) => {
      g.setAttribute("transform", `matrix(${cam.a} 0 0 ${cam.d} ${cam.e} ${cam.f})`);
      const scale = Math.abs(cam.a);
      const hit = String(HIT_RADIUS_PX / scale);
      const vis = String(VIS_RADIUS_PX / scale);
      for (const c of g.querySelectorAll("circle.pin-hit")) c.setAttribute("r", hit);
      for (const c of g.querySelectorAll("circle.pin-vis")) c.setAttribute("r", vis);
    });
  }, [ports, wrapRef]);

  // Resting dots: while the pointer is over a component (the viewer's own
  // element, underneath this overlay), its pins show faint dots so the
  // wire gesture has a visible origin. Imperative attribute toggling — a
  // mousemove must not re-render.
  useEffect(() => {
    const wrap = wrapRef.current;
    const g = gRef.current;
    if (!wrap || !g) return;
    let current: string | null = null;
    const move = (e: MouseEvent) => {
      const comp = (e.target as Element).closest?.("[data-schematic-component-id]");
      const id = comp?.getAttribute("data-schematic-component-id");
      const path = id ? (view.id_map[id] ?? null) : null;
      if (path === current) return;
      current = path;
      for (const node of g.querySelectorAll("g[data-port-path]")) {
        if (path && node.getAttribute("data-port-path") === path) {
          node.setAttribute("data-resting", "");
        } else {
          node.removeAttribute("data-resting");
        }
      }
    };
    wrap.addEventListener("mousemove", move);
    return () => wrap.removeEventListener("mousemove", move);
  }, [view, wrapRef, ports]);

  if (ports.length === 0) return null;

  return (
    <svg className="pointer-events-none absolute inset-0 h-full w-full overflow-visible">
      {/* The wire-drag preview, in screen space (outside the camera group). */}
      <line ref={lineRef} className="wire-preview" style={{ display: "none" }} />
      <g ref={gRef}>
        {ports.map((p) => (
          <g key={p.id} data-port-path={p.path} data-generated={p.generated || undefined}>
            <circle
              className="pin-hit"
              cx={p.x}
              cy={p.y}
              data-port-path={p.path}
              data-port-pin={p.pin}
              role="button"
              aria-label={`pin ${p.pin} of ${p.path}`}
              onMouseDown={(e) => {
                // A press can become a wire drag; below the threshold it
                // stays a click. Label mode owns clicks outright.
                if (e.button !== 0 || labelArmed) return;
                dragRef.current = {
                  path: p.path,
                  pin: p.pin,
                  startX: e.clientX,
                  startY: e.clientY,
                  active: false,
                };
              }}
              onClick={(e) => {
                e.stopPropagation();
                if (suppressClickRef.current) {
                  suppressClickRef.current = false;
                  return;
                }
                if (labelArmed && onPortLabel) {
                  onPortLabel(
                    { path: p.path, pin: p.pin },
                    { x: e.clientX, y: e.clientY },
                  );
                } else {
                  onPortClick(p.path, e.shiftKey);
                }
              }}
            >
              <title>
                {p.pin ? `${p.pin} — ${p.path}` : p.path}
                {p.reason ? ` · ${p.reason}` : ""}
              </title>
            </circle>
            <circle className="pin-vis" cx={p.x} cy={p.y} />
          </g>
        ))}
      </g>
    </svg>
  );
}
