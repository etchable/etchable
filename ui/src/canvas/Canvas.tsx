import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { Diag, SchematicDoc } from "../types";
import { layoutSchematic } from "./layout";
import type { LayoutBox, PinPoint, SchematicLayout } from "./layout";
import { netColor, routeNets } from "./nets";

const ACCENT = "#7aa2ff";
const MIN_ZOOM = 0.05;
const MAX_ZOOM = 8;

type View = { x: number; y: number; k: number };

type DragState =
  | { mode: "pan"; lastX: number; lastY: number }
  | { mode: "marquee"; startSX: number; startSY: number; shift: boolean; moved: boolean };

type Tooltip = { sx: number; sy: number; lines: string[] };

type CanvasProps = {
  schematic: SchematicDoc | null;
  source: string | null;
  dimmed: boolean;
  diagnostics: Diag[];
  selection: string[];
  onSelectionChange: (paths: string[]) => void;
};

function clamp(v: number, lo: number, hi: number): number {
  return Math.min(hi, Math.max(lo, v));
}

function fileMatches(a: string | null | undefined, b: string | undefined): boolean {
  if (!a || !b) return false;
  return a === b || a.endsWith("/" + b) || b.endsWith("/" + a);
}

export default function Canvas(props: CanvasProps) {
  const { schematic, source, dimmed, diagnostics, selection, onSelectionChange } = props;

  const containerRef = useRef<HTMLDivElement>(null);
  const [view, setView] = useState<View>({ x: 0, y: 0, k: 1 });
  const viewRef = useRef(view);
  viewRef.current = view;

  const [hoverBox, setHoverBox] = useState<string | null>(null);
  const [hoverNet, setHoverNet] = useState<string | null>(null);
  const [marquee, setMarquee] = useState<{ x: number; y: number; w: number; h: number } | null>(null);
  const [tooltip, setTooltip] = useState<Tooltip | null>(null);
  const [grabbing, setGrabbing] = useState(false);
  const [spaceHeld, setSpaceHeld] = useState(false);

  const dragRef = useRef<DragState | null>(null);
  const spaceRef = useRef(false);
  const selectionRef = useRef(selection);
  selectionRef.current = selection;
  const onSelectRef = useRef(onSelectionChange);
  onSelectRef.current = onSelectionChange;

  const layout: SchematicLayout | null = useMemo(
    () => (schematic ? layoutSchematic(schematic) : null),
    [schematic],
  );
  const nets = useMemo(
    () => (schematic && layout ? routeNets(schematic, layout) : []),
    [schematic, layout],
  );

  const pinsByComponent = useMemo(() => {
    const m = new Map<string, PinPoint[]>();
    if (layout) {
      for (const pp of layout.pinPoints.values()) {
        const list = m.get(pp.componentPath);
        if (list) list.push(pp);
        else m.set(pp.componentPath, [pp]);
      }
    }
    return m;
  }, [layout]);

  // Diagnostics badges: component path -> matching error/warning diags.
  const diagsByBox = useMemo(() => {
    const m = new Map<string, Diag[]>();
    if (!schematic || !layout) return m;
    const active = diagnostics.filter(
      (d) => !d.suppressed && (d.severity === "error" || d.severity === "warning"),
    );
    if (active.length === 0) return m;
    for (const box of layout.boxes.values()) {
      if (box.kind !== "component") continue;
      const src = schematic.instances[box.path]?.source_file;
      const hits = active.filter((d) => fileMatches(src, d.file));
      if (hits.length > 0) m.set(box.path, hits);
    }
    return m;
  }, [schematic, layout, diagnostics]);

  const selSet = useMemo(() => new Set(selection), [selection]);

  // ---- viewport ------------------------------------------------------------

  const fitView = useCallback(() => {
    const el = containerRef.current;
    if (!el || !layout || layout.boxes.size === 0) return;
    const b = layout.bounds;
    const margin = 60;
    const k = clamp(
      Math.min((el.clientWidth - margin * 2) / b.w, (el.clientHeight - margin * 2) / b.h),
      MIN_ZOOM,
      1.5,
    );
    setView({
      k,
      x: (el.clientWidth - b.w * k) / 2 - b.x * k,
      y: (el.clientHeight - b.h * k) / 2 - b.y * k,
    });
  }, [layout]);

  // Zoom-to-fit on first build / board change; preserve viewport on rebuilds.
  const savedViews = useRef(new Map<string, View>());
  const prevSourceRef = useRef<string | null>(null);
  const fittedForRef = useRef<string | null>(null);

  useEffect(() => {
    if (source !== prevSourceRef.current) {
      if (prevSourceRef.current !== null) {
        savedViews.current.set(prevSourceRef.current, viewRef.current);
      }
      prevSourceRef.current = source;
      fittedForRef.current = null;
    }
    if (!layout || !source || fittedForRef.current === source) return;
    const saved = savedViews.current.get(source);
    if (saved) setView(saved);
    else fitView();
    fittedForRef.current = source;
  }, [source, layout, fitView]);

  // ---- wheel: zoom to cursor (ctrl/pinch) or pan (two-finger scroll) -------

  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      const rect = el.getBoundingClientRect();
      const sx = e.clientX - rect.left;
      const sy = e.clientY - rect.top;
      setView((v) => {
        if (e.ctrlKey || e.metaKey) {
          const k = clamp(v.k * Math.exp(-e.deltaY * 0.01), MIN_ZOOM, MAX_ZOOM);
          return { k, x: sx - ((sx - v.x) * k) / v.k, y: sy - ((sy - v.y) * k) / v.k };
        }
        return { ...v, x: v.x - e.deltaX, y: v.y - e.deltaY };
      });
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  }, []);

  // ---- keyboard: space = pan, esc = clear selection ------------------------

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
      if (e.key === " " && !isTyping()) {
        spaceRef.current = true;
        setSpaceHeld(true);
        e.preventDefault();
      } else if (e.key === "Escape" && !isTyping()) {
        if (selectionRef.current.length > 0) onSelectRef.current([]);
      }
    };
    const up = (e: KeyboardEvent) => {
      if (e.key === " ") {
        spaceRef.current = false;
        setSpaceHeld(false);
      }
    };
    window.addEventListener("keydown", down);
    window.addEventListener("keyup", up);
    return () => {
      window.removeEventListener("keydown", down);
      window.removeEventListener("keyup", up);
    };
  }, []);

  // ---- pointer interactions ------------------------------------------------

  const toWorld = useCallback((sx: number, sy: number) => {
    const v = viewRef.current;
    return { x: (sx - v.x) / v.k, y: (sy - v.y) / v.k };
  }, []);

  const marqueeFromEvent = useCallback(
    (drag: { startSX: number; startSY: number }, e: { clientX: number; clientY: number }) => {
      const el = containerRef.current;
      if (!el) return null;
      const rect = el.getBoundingClientRect();
      const a = toWorld(drag.startSX - rect.left, drag.startSY - rect.top);
      const b = toWorld(e.clientX - rect.left, e.clientY - rect.top);
      return {
        x: Math.min(a.x, b.x),
        y: Math.min(a.y, b.y),
        w: Math.abs(a.x - b.x),
        h: Math.abs(a.y - b.y),
      };
    },
    [toWorld],
  );

  const onPointerDown = (e: React.PointerEvent<HTMLDivElement>) => {
    if (e.button === 1 || (e.button === 0 && spaceRef.current)) {
      dragRef.current = { mode: "pan", lastX: e.clientX, lastY: e.clientY };
      setGrabbing(true);
      e.currentTarget.setPointerCapture(e.pointerId);
      e.preventDefault();
    } else if (e.button === 0) {
      dragRef.current = {
        mode: "marquee",
        startSX: e.clientX,
        startSY: e.clientY,
        shift: e.shiftKey,
        moved: false,
      };
      e.currentTarget.setPointerCapture(e.pointerId);
    }
  };

  const onPointerMove = (e: React.PointerEvent<HTMLDivElement>) => {
    const drag = dragRef.current;
    if (!drag) return;
    if (drag.mode === "pan") {
      const dx = e.clientX - drag.lastX;
      const dy = e.clientY - drag.lastY;
      drag.lastX = e.clientX;
      drag.lastY = e.clientY;
      setView((v) => ({ ...v, x: v.x + dx, y: v.y + dy }));
    } else {
      if (!drag.moved && Math.hypot(e.clientX - drag.startSX, e.clientY - drag.startSY) > 3) {
        drag.moved = true;
      }
      if (drag.moved) setMarquee(marqueeFromEvent(drag, e));
    }
  };

  const onPointerUp = (e: React.PointerEvent<HTMLDivElement>) => {
    const drag = dragRef.current;
    dragRef.current = null;
    setGrabbing(false);
    if (!drag || drag.mode === "pan") return;
    setMarquee(null);
    if (drag.moved) {
      const m = marqueeFromEvent(drag, e);
      if (!m || !layout) return;
      const hits: string[] = [];
      for (const box of layout.boxes.values()) {
        if (box.x < m.x + m.w && box.x + box.w > m.x && box.y < m.y + m.h && box.y + box.h > m.y) {
          hits.push(box.path);
        }
      }
      if (drag.shift) {
        const merged = new Set([...selection, ...hits]);
        onSelectionChange([...merged]);
      } else {
        onSelectionChange(hits);
      }
    } else if (!drag.shift) {
      onSelectionChange([]);
    }
  };

  const clickBox = (e: React.MouseEvent, path: string) => {
    e.stopPropagation();
    if (e.shiftKey) {
      onSelectionChange(
        selection.includes(path) ? selection.filter((p) => p !== path) : [...selection, path],
      );
    } else {
      onSelectionChange([path]);
    }
  };

  const clickNet = (e: React.MouseEvent, name: string) => {
    e.stopPropagation();
    if (e.shiftKey) {
      onSelectionChange(
        selection.includes(name) ? selection.filter((p) => p !== name) : [...selection, name],
      );
    } else {
      onSelectionChange([name]);
    }
  };

  const stopPointer = (e: React.PointerEvent | React.MouseEvent) => e.stopPropagation();

  const showBadgeTooltip = (box: LayoutBox, diags: Diag[]) => {
    const v = viewRef.current;
    const lines = diags.slice(0, 3).map((d) => `${d.severity}: ${d.message}`);
    if (diags.length > 3) lines.push(`+${diags.length - 3} more`);
    setTooltip({
      sx: (box.x + box.w) * v.k + v.x,
      sy: box.y * v.k + v.y,
      lines,
    });
  };

  // ---- rendering -----------------------------------------------------------

  const netKindByName = useMemo(() => {
    const m = new Map<string, string>();
    if (schematic) {
      for (const n of Object.values(schematic.nets)) m.set(n.name, n.kind);
    }
    return m;
  }, [schematic]);

  const moduleBoxes: LayoutBox[] = [];
  const componentBoxes: LayoutBox[] = [];
  if (layout) {
    for (const b of layout.boxes.values()) {
      (b.kind === "module" ? moduleBoxes : componentBoxes).push(b);
    }
    moduleBoxes.sort((a, b) => a.depth - b.depth);
  }

  const cursor = grabbing ? "grabbing" : spaceHeld ? "grab" : "default";

  const selChipText = useMemo(() => {
    if (selection.length === 0) return "";
    const joined = selection.join(", ");
    return joined.length > 64 ? joined.slice(0, 64) + "…" : joined;
  }, [selection]);

  return (
    <div
      ref={containerRef}
      className="canvas-wrap"
      style={{ cursor }}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onDoubleClick={fitView}
    >
      <svg className="canvas-svg">
        <g
          transform={`translate(${view.x},${view.y}) scale(${view.k})`}
          opacity={dimmed ? 0.5 : 1}
        >
          {/* module containers, outermost first */}
          {moduleBoxes.map((b) => {
            const selected = selSet.has(b.path);
            const hovered = hoverBox === b.path;
            return (
              <g
                key={b.path}
                className="box module-box"
                style={selected ? { filter: `drop-shadow(0 0 6px ${ACCENT}99)` } : undefined}
                onPointerDown={stopPointer}
                onDoubleClick={stopPointer}
                onClick={(e) => clickBox(e, b.path)}
                onPointerEnter={() => setHoverBox(b.path)}
                onPointerLeave={() => setHoverBox(null)}
              >
                <rect
                  x={b.x}
                  y={b.y}
                  width={b.w}
                  height={b.h}
                  rx={4}
                  fill="rgba(255,255,255,0.018)"
                  stroke={selected ? ACCENT : hovered ? "#4a5668" : "#28303d"}
                  strokeWidth={selected ? 1.5 : 1}
                  vectorEffect="non-scaling-stroke"
                />
                <line
                  x1={b.x}
                  y1={b.y + 22}
                  x2={b.x + b.w}
                  y2={b.y + 22}
                  stroke={selected ? ACCENT : "#28303d"}
                  strokeWidth={1}
                  vectorEffect="non-scaling-stroke"
                />
                <text x={b.x + 8} y={b.y + 15} className="t-mod-name" fill="#c7d0dc">
                  {b.name}
                </text>
                {b.typeName !== "<root>" && (
                  <text x={b.x + 8 + b.name.length * 7.5 + 8} y={b.y + 15} className="t-dim">
                    {b.typeName}
                  </text>
                )}
              </g>
            );
          })}

          {/* net wires */}
          {nets.map((net) => {
            const active = hoverNet === net.name || selSet.has(net.name);
            return (
              <g
                key={net.name}
                className="net"
                onPointerDown={stopPointer}
                onClick={(e) => clickNet(e, net.name)}
                onPointerEnter={() => setHoverNet(net.name)}
                onPointerLeave={() => setHoverNet(null)}
              >
                {net.segments.map((s, i) => (
                  <line
                    key={`hit${i}`}
                    x1={s.x1}
                    y1={s.y1}
                    x2={s.x2}
                    y2={s.y2}
                    stroke="rgba(0,0,0,0)"
                    strokeWidth={10 / view.k}
                    pointerEvents="stroke"
                  />
                ))}
                {active &&
                  net.segments.map((s, i) => (
                    <line
                      key={`glow${i}`}
                      x1={s.x1}
                      y1={s.y1}
                      x2={s.x2}
                      y2={s.y2}
                      stroke={net.color}
                      strokeWidth={7}
                      opacity={0.28}
                      strokeLinecap="round"
                      vectorEffect="non-scaling-stroke"
                      pointerEvents="none"
                    />
                  ))}
                {net.segments.map((s, i) => (
                  <line
                    key={i}
                    x1={s.x1}
                    y1={s.y1}
                    x2={s.x2}
                    y2={s.y2}
                    stroke={net.color}
                    strokeWidth={active ? 2 : 1.25}
                    opacity={active ? 1 : 0.6}
                    vectorEffect="non-scaling-stroke"
                    pointerEvents="none"
                  />
                ))}
                {net.junctions.map((j, i) => (
                  <circle
                    key={`j${i}`}
                    cx={j.x}
                    cy={j.y}
                    r={2.5}
                    fill={net.color}
                    opacity={active ? 1 : 0.75}
                    pointerEvents="none"
                  />
                ))}
              </g>
            );
          })}

          {/* component boxes */}
          {componentBoxes.map((b) => {
            const selected = selSet.has(b.path);
            const hovered = hoverBox === b.path;
            const pins = pinsByComponent.get(b.path) ?? [];
            const badges = diagsByBox.get(b.path);
            const hasError = badges?.some((d) => d.severity === "error") ?? false;
            return (
              <g
                key={b.path}
                className="box comp-box"
                style={selected ? { filter: `drop-shadow(0 0 6px ${ACCENT}99)` } : undefined}
                onPointerDown={stopPointer}
                onDoubleClick={stopPointer}
                onClick={(e) => clickBox(e, b.path)}
                onPointerEnter={() => setHoverBox(b.path)}
                onPointerLeave={() => setHoverBox(null)}
              >
                <rect
                  x={b.x}
                  y={b.y}
                  width={b.w}
                  height={b.h}
                  rx={3}
                  fill="#151a22"
                  stroke={selected ? ACCENT : hovered ? "#55617a" : "#323b4a"}
                  strokeWidth={selected ? 1.5 : 1}
                  vectorEffect="non-scaling-stroke"
                />
                <text x={b.x + b.w / 2} y={b.y + b.h / 2 - 7} textAnchor="middle" className="t-refdes">
                  {b.refdes ?? b.name}
                </text>
                <text x={b.x + b.w / 2} y={b.y + b.h / 2 + 6} textAnchor="middle" className="t-dim">
                  {b.typeName}
                </text>
                {b.value && (
                  <text x={b.x + b.w / 2} y={b.y + b.h / 2 + 18} textAnchor="middle" className="t-dim">
                    {b.value}
                  </text>
                )}

                {/* pin stubs, dots, names, net tags */}
                {pins.map((p) => {
                  const left = p.side === "left";
                  const kind = p.net ? netKindByName.get(p.net) ?? "Net" : "Net";
                  const tagColor = netColor(kind);
                  return (
                    <g key={p.name}>
                      <line
                        x1={p.edgeX}
                        y1={p.edgeY}
                        x2={p.x}
                        y2={p.y}
                        stroke="#5a6472"
                        strokeWidth={1}
                        vectorEffect="non-scaling-stroke"
                      />
                      <circle cx={p.x} cy={p.y} r={2} fill="#8b95a3" />
                      <text
                        x={left ? p.edgeX + 4 : p.edgeX - 4}
                        y={p.edgeY + 2.5}
                        textAnchor={left ? "start" : "end"}
                        className="t-pin"
                      >
                        {p.name}
                      </text>
                      {p.net && (
                        <text
                          x={left ? p.x - 4 : p.x + 4}
                          y={p.y - 4}
                          textAnchor={left ? "end" : "start"}
                          className="t-nettag"
                          fill={tagColor}
                          onClick={(e) => clickNet(e, p.net as string)}
                        >
                          {p.net}
                        </text>
                      )}
                    </g>
                  );
                })}

                {/* diagnostics badge */}
                {badges && (
                  <g
                    className="diag-badge"
                    onPointerEnter={() => showBadgeTooltip(b, badges)}
                    onPointerLeave={() => setTooltip(null)}
                  >
                    <circle
                      cx={b.x + b.w - 1}
                      cy={b.y + 1}
                      r={7}
                      fill={hasError ? "#f87171" : "#fbbf24"}
                      stroke="#0e1116"
                      strokeWidth={1.5}
                    />
                    <text
                      x={b.x + b.w - 1}
                      y={b.y + 4}
                      textAnchor="middle"
                      className="t-badge"
                    >
                      {badges.length}
                    </text>
                  </g>
                )}
              </g>
            );
          })}

          {/* marquee */}
          {marquee && (
            <rect
              x={marquee.x}
              y={marquee.y}
              width={marquee.w}
              height={marquee.h}
              fill="rgba(122,162,255,0.08)"
              stroke={ACCENT}
              strokeDasharray="4 3"
              strokeWidth={1}
              vectorEffect="non-scaling-stroke"
              pointerEvents="none"
            />
          )}
        </g>
      </svg>

      {dimmed && <div className="canvas-toast">build failing — see Problems</div>}

      {selection.length > 0 && (
        <div className="sel-chip" title={selection.join("\n")}>
          <span className="sel-chip-count">{selection.length} selected</span>
          <span className="sel-chip-paths"> · {selChipText}</span>
        </div>
      )}

      {tooltip && (
        <div
          className="canvas-tooltip"
          style={{ left: tooltip.sx + 10, top: tooltip.sy + 10 }}
        >
          {tooltip.lines.map((l, i) => (
            <div key={i}>{l}</div>
          ))}
        </div>
      )}
    </div>
  );
}
