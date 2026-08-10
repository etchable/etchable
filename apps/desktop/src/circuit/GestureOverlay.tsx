// The gesture overlay (decision 0009 §7): pin targets, hover affordances,
// and the drag-to-wire gesture, rendered above the viewer and glued to its
// camera. Reworked per the UX review: pins show a faint RESTING dot while
// the pointer is over their component (wiring is discoverable, not
// secret), hit targets are larger than the visible ring, tooltips carry
// the editability reason, and wire drops report their screen point so
// confirmations can anchor at the gesture.

import { useEffect, useMemo, useRef } from "react";
import { observeCamera, readCamera } from "./camera";
import type { BuildView } from "../types";

/** Hit target and visible ring, in constant screen px. */
const HIT_RADIUS_PX = 10;
const VIS_RADIUS_PX = 6;
const DRAG_THRESHOLD_PX = 5;
/** Fraction of the hit disc that still overlaps the pin after it is pushed
 * outward — enough to grab the pin itself, little enough that the body (the
 * move handle) stays clear. */
const HIT_INSET = 0.2;

type PortTarget = {
  id: string;
  x: number;
  y: number;
  /** Unit vector pointing away from the body, along the pin's lead. Ports sit
   * ON the body edge, so a hit disc centered on the pin reaches back over the
   * symbol; offsetting it outward keeps the body pressable (that's the move
   * handle) while the target still sits where the wire will run. */
  ox: number;
  oy: number;
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
  /** Selected instance paths, for the multi-selection frame. */
  selection: string[];
  onPortClick: (path: string, shiftKey: boolean) => void;
  /** The component under the pointer (null when over empty canvas), so
   * keyboard verbs can act on what is hovered the way KiCad does. */
  onHoverComponent?: (path: string | null) => void;
  /** When set, pin clicks label instead of selecting (phase 2). */
  labelArmed?: boolean;
  /** Wire tool armed (`W`): click a pin, click another, they connect. KiCad's
   * modal habit, alongside the existing pin-to-pin drag. */
  wireArmed?: boolean;
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
  const {
    view,
    wrapRef,
    selection,
    onPortClick,
    onHoverComponent,
    labelArmed,
    wireArmed,
    onPortLabel,
    onWireConnect,
    onWireToNet,
  } = props;
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
  // Wire tool: the pin awaiting its partner (first click of the pair).
  const pendingRef = useRef<{ path: string; pin: string; x: number; y: number } | null>(null);

  useEffect(() => {
    const move = (e: MouseEvent) => {
      const drag = dragRef.current;
      const wrap = wrapRef.current;
      const line = lineRef.current;
      // Wire tool: rubber-band from the pending pin to the cursor. The anchor
      // is recomputed from the pin's own coordinates so it stays put across
      // pan and zoom.
      const pending = pendingRef.current;
      if (pending && wrap && line && !drag) {
        const cam = readCamera(wrap);
        const rect = wrap.getBoundingClientRect();
        if (cam) {
          line.style.display = "block";
          line.setAttribute("x1", String(cam.a * pending.x + cam.e));
          line.setAttribute("y1", String(cam.d * pending.y + cam.f));
          line.setAttribute("x2", String(e.clientX - rect.left));
          line.setAttribute("y2", String(e.clientY - rect.top));
        }
        return;
      }
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
      const dropPort = portAtRef.current(e.clientX, e.clientY);
      if (dropPort) {
        if (dropPort.path !== drag.path || dropPort.pin !== drag.pin) {
          onWireConnect?.(
            { path: drag.path, pin: drag.pin },
            { path: dropPort.path, pin: dropPort.pin },
            { x: e.clientX, y: e.clientY },
          );
        }
        return;
      }
      const el = document.elementFromPoint(e.clientX, e.clientY);
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
    const armedDown = (e: MouseEvent) => {
      if (!wireArmed || e.button !== 0) return;
      const port = portAtRef.current(e.clientX, e.clientY);
      if (!port) return;
      // A press near a pin can become a drag to another pin; below the
      // threshold it stays a click and the pending-pin logic handles it.
      dragRef.current = {
        path: port.path,
        pin: port.pin,
        startX: e.clientX,
        startY: e.clientY,
        active: false,
      };
    };
    const armedClick = (e: MouseEvent) => {
      if (!(wireArmed || labelArmed) || e.button !== 0) return;
      if (suppressClickRef.current) {
        suppressClickRef.current = false;
        return;
      }
      const port = portAtRef.current(e.clientX, e.clientY);
      if (!port) return;
      e.preventDefault();
      e.stopPropagation();
      if (labelArmed && onPortLabel) {
        onPortLabel({ path: port.path, pin: port.pin }, { x: e.clientX, y: e.clientY });
        return;
      }
      const pending = pendingRef.current;
      const line = lineRef.current;
      if (!pending) {
        pendingRef.current = { path: port.path, pin: port.pin, x: port.x, y: port.y };
        return;
      }
      pendingRef.current = null;
      if (line) line.style.display = "none";
      if (pending.path !== port.path || pending.pin !== port.pin) {
        onWireConnect?.(
          { path: pending.path, pin: pending.pin },
          { path: port.path, pin: port.pin },
          { x: e.clientX, y: e.clientY },
        );
      }
    };
    const cancelPending = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || !pendingRef.current) return;
      pendingRef.current = null;
      const line = lineRef.current;
      if (line) line.style.display = "none";
    };
    const wrap = wrapRef.current;
    wrap?.addEventListener("mousedown", armedDown, true);
    wrap?.addEventListener("click", armedClick, true);
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
    window.addEventListener("keydown", cancelPending);
    return () => {
      wrap?.removeEventListener("mousedown", armedDown, true);
      wrap?.removeEventListener("click", armedClick, true);
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
      window.removeEventListener("keydown", cancelPending);
    };
  }, [view, wrapRef, onWireConnect, onWireToNet, wireArmed, labelArmed, onPortLabel]);

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
    // Body centers, to point each pin's hit target away from its symbol.
    const bodyCenter = new Map<string, { x: number; y: number }>();
    for (const el of view.circuit_json) {
      if (el.type !== "schematic_component") continue;
      const id = el.schematic_component_id;
      const center = el.center as { x: number; y: number } | undefined;
      if (typeof id === "string" && center) bodyCenter.set(id, center);
    }
    const out: PortTarget[] = [];
    for (const el of view.circuit_json) {
      if (el.type !== "schematic_port") continue;
      const id = el.schematic_port_id;
      const center = el.center as { x: number; y: number } | undefined;
      if (typeof id !== "string" || !center) continue;
      const path = view.id_map[id];
      if (!path) continue;
      const compId = el.schematic_component_id;
      const body = typeof compId === "string" ? bodyCenter.get(compId) : undefined;
      let ox = 0;
      let oy = 0;
      if (body) {
        const dx = center.x - body.x;
        const dy = center.y - body.y;
        // Pins leave along one axis; the dominant component is the lead.
        if (Math.abs(dx) >= Math.abs(dy)) {
          ox = Math.sign(dx);
        } else {
          oy = Math.sign(dy);
        }
      }
      const srcId = el.source_port_id;
      const edit = view.editability?.instances[path];
      const generated = edit ? !edit.editable && !edit.anchor : false;
      out.push({
        id,
        x: center.x,
        y: center.y,
        ox,
        oy,
        pin: (typeof srcId === "string" && nameBySourcePort.get(srcId)) || "",
        path,
        generated,
        reason: generated ? (edit?.reason ?? null) : null,
      });
    }
    return out;
  }, [view]);

  /**
   * Nearest port to a screen point, or null past the threshold.
   *
   * Pin targeting is done here in JS rather than by letting the SVG discs take
   * the events: those discs are tiny fractional-radius circles inside a heavily
   * scaled group, and their hit regions do not match their geometry — clicks
   * 15px apart on two different pins both resolved to whichever disc came first
   * in the DOM, which is what made pins, wires, and symbol bodies all
   * unreachable. Measuring distances ourselves is exact at any zoom, and it
   * gives the snap-to-nearest-pin behaviour an EDA canvas wants anyway.
   */
  const portAtRef = useRef<(clientX: number, clientY: number) => PortTarget | null>(
    () => null,
  );
  portAtRef.current = (clientX: number, clientY: number) => {
    const wrap = wrapRef.current;
    if (!wrap) return null;
    const cam = readCamera(wrap);
    if (!cam) return null;
    const rect = wrap.getBoundingClientRect();
    const sx = clientX - rect.left;
    const sy = clientY - rect.top;
    let best: PortTarget | null = null;
    let bestDist = HIT_RADIUS_PX;
    for (const p of ports) {
      const px = cam.a * p.x + cam.e;
      const py = cam.d * p.y + cam.f;
      const d = Math.hypot(px - sx, py - sy);
      if (d <= bestDist) {
        bestDist = d;
        best = p;
      }
    }
    return best;
  };

  /**
   * One frame around everything selected, the way a design tool shows a
   * multi-selection: it tells you what a drag is about to move. Single
   * selections already read clearly from their own halo, so the frame earns
   * its ink only from two up.
   */
  const selectionFrame = useMemo(() => {
    if (selection.length < 2) return null;
    const wanted = new Set(selection);
    let x0 = Infinity;
    let y0 = Infinity;
    let x1 = -Infinity;
    let y1 = -Infinity;
    for (const el of view.circuit_json) {
      if (el.type !== "schematic_component") continue;
      const id = el.schematic_component_id;
      const path = typeof id === "string" ? view.id_map[id] : undefined;
      if (!path || !wanted.has(path)) continue;
      const c = el.center as { x: number; y: number } | undefined;
      const size = el.size as { width: number; height: number } | undefined;
      if (!c || !size) continue;
      x0 = Math.min(x0, c.x - size.width / 2);
      x1 = Math.max(x1, c.x + size.width / 2);
      y0 = Math.min(y0, c.y - size.height / 2);
      y1 = Math.max(y1, c.y + size.height / 2);
    }
    if (!Number.isFinite(x0) || !Number.isFinite(y0)) return null;
    const pad = 0.15;
    return { x: x0 - pad, y: y0 - pad, w: x1 - x0 + pad * 2, h: y1 - y0 + pad * 2 };
  }, [selection, view]);

  // Glue the target layer to the viewer camera: one transform on the group,
  // and per-frame radii so hit targets and rings keep constant screen size.
  useEffect(() => {
    const wrap = wrapRef.current;
    const g = gRef.current;
    if (!wrap || !g || ports.length === 0) return;
    return observeCamera(wrap, (cam) => {
      g.setAttribute("transform", `matrix(${cam.a} 0 0 ${cam.d} ${cam.e} ${cam.f})`);
      const scale = Math.abs(cam.a);
      const hitR = HIT_RADIUS_PX / scale;
      const vis = String(VIS_RADIUS_PX / scale);
      const hit = String(hitR);
      for (const c of g.querySelectorAll("circle.pin-hit")) {
        c.setAttribute("r", hit);
        // Push the disc outward along the lead so it clears the body. It stays
        // tangent-ish to the pin (HIT_INSET of it still covers the pin itself)
        // so wiring from the pin keeps working at any zoom.
        const px = Number(c.getAttribute("data-px"));
        const py = Number(c.getAttribute("data-py"));
        const ox = Number(c.getAttribute("data-ox"));
        const oy = Number(c.getAttribute("data-oy"));
        c.setAttribute("cx", String(px + ox * hitR * (1 - HIT_INSET)));
        c.setAttribute("cy", String(py + oy * hitR * (1 - HIT_INSET)));
      }
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
      const el = e.target as Element;
      const comp = el.closest?.("[data-schematic-component-id]");
      const id = comp?.getAttribute("data-schematic-component-id");
      // Over a pin target the disc itself names the owning component, so
      // hovering a pin still targets its symbol — the way KiCad highlights the
      // symbol when the cursor is on one of its pins.
      const path = id
        ? (view.id_map[id] ?? null)
        : (el.closest?.("[data-port-path]")?.getAttribute("data-port-path") ?? null);
      if (path === current) return;
      current = path;
      onHoverComponent?.(path);
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
  }, [view, wrapRef, ports, onHoverComponent]);

  if (ports.length === 0) return null;

  return (
    <svg
      className={`pointer-events-none absolute inset-0 h-full w-full overflow-visible ${
        wireArmed || labelArmed ? "tool-armed" : ""
      }`}
    >
      {/* The wire-drag preview, in screen space (outside the camera group). */}
      <line ref={lineRef} className="wire-preview" style={{ display: "none" }} />
      <g ref={gRef}>
        {selectionFrame && (
          <rect
            className="sel-frame"
            x={selectionFrame.x}
            y={selectionFrame.y}
            width={selectionFrame.w}
            height={selectionFrame.h}
          />
        )}
        {ports.map((p) => (
          <g key={p.id} data-port-path={p.path} data-generated={p.generated || undefined}>
            <circle
              className="pin-hit"
              cx={p.x}
              cy={p.y}
              data-px={p.x}
              data-py={p.y}
              data-ox={p.ox}
              data-oy={p.oy}
              data-port-path={p.path}
              data-port-pin={p.pin}
              aria-hidden
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
                if (wireArmed) {
                  const pending = pendingRef.current;
                  const line = lineRef.current;
                  if (!pending) {
                    pendingRef.current = { path: p.path, pin: p.pin, x: p.x, y: p.y };
                    return;
                  }
                  pendingRef.current = null;
                  if (line) line.style.display = "none";
                  if (pending.path !== p.path || pending.pin !== p.pin) {
                    onWireConnect?.(
                      { path: pending.path, pin: pending.pin },
                      { path: p.path, pin: p.pin },
                      { x: e.clientX, y: e.clientY },
                    );
                  }
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
