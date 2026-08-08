import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type CSSProperties,
  type PointerEvent as ReactPointerEvent,
  type ReactNode,
} from "react";

/** The hero device: marching-ants border + corner handles, as if the
    child were an object selected in its own editor. Use once per page,
    on the most important object.

    It's also a toy: grab a handle or an edge and the child stretches
    with the pointer; on release it springs back to rest. Everything is
    transform/inset based — layout around the box never shifts. */

type Sides = { top: number; right: number; bottom: number; left: number };
type SideKey = keyof Sides;

const REST: Sides = { top: 0, right: 0, bottom: 0, left: 0 };
const SIDE_KEYS: SideKey[] = ["top", "right", "bottom", "left"];

// Underdamped spring (ζ ≈ 0.4): chases the pointer with a slight
// rubbery lag while dragging, overshoots and wobbles on the way home.
const STIFFNESS = 800;
const DAMPING = 22;
const SETTLED = 0.05;

const CORNER = 24; // square hit target centred on each corner handle
const EDGE = 12; // hit strip straddling each border edge

type Grip = {
  sides: SideKey[];
  cursor: string;
  style: CSSProperties;
  handle?: boolean;
};

const GRIPS: Grip[] = [
  { sides: ["top", "left"], cursor: "nwse-resize", handle: true, style: { top: -CORNER / 2, left: -CORNER / 2 } },
  { sides: ["top", "right"], cursor: "nesw-resize", handle: true, style: { top: -CORNER / 2, right: -CORNER / 2 } },
  { sides: ["bottom", "left"], cursor: "nesw-resize", handle: true, style: { bottom: -CORNER / 2, left: -CORNER / 2 } },
  { sides: ["bottom", "right"], cursor: "nwse-resize", handle: true, style: { bottom: -CORNER / 2, right: -CORNER / 2 } },
  { sides: ["top"], cursor: "ns-resize", style: { top: -EDGE / 2, left: CORNER / 2, right: CORNER / 2, height: EDGE } },
  { sides: ["bottom"], cursor: "ns-resize", style: { bottom: -EDGE / 2, left: CORNER / 2, right: CORNER / 2, height: EDGE } },
  { sides: ["left"], cursor: "ew-resize", style: { left: -EDGE / 2, top: CORNER / 2, bottom: CORNER / 2, width: EDGE } },
  { sides: ["right"], cursor: "ew-resize", style: { right: -EDGE / 2, top: CORNER / 2, bottom: CORNER / 2, width: EDGE } },
];

export function SelectionBox({ children }: { children: ReactNode }) {
  const rootRef = useRef<HTMLDivElement>(null);
  // How far each side of the frame currently sits from rest, in px.
  const [sides, setSides] = useState<Sides>(REST);

  const pos = useRef<Sides>({ ...REST });
  const vel = useRef<Sides>({ ...REST });
  const target = useRef<Sides>({ ...REST });
  const base = useRef<{ w: number; h: number } | null>(null);
  const drag = useRef<{ id: number; x: number; y: number; sides: SideKey[]; instant: boolean } | null>(null);
  const raf = useRef(0);
  const lastT = useRef(0);

  const tick = useCallback((now: number) => {
    raf.current = 0;
    const dt = Math.min(Math.max((now - lastT.current) / 1000, 0), 1 / 30);
    lastT.current = now;
    let settled = drag.current === null;
    const next = { ...pos.current };
    for (const k of SIDE_KEYS) {
      const t = target.current[k];
      let v = vel.current[k] + (STIFFNESS * (t - next[k]) - DAMPING * vel.current[k]) * dt;
      let x = next[k] + v * dt;
      if (Math.abs(t - x) < SETTLED && Math.abs(v) < SETTLED) {
        x = t;
        v = 0;
      } else {
        settled = false;
      }
      next[k] = x;
      vel.current[k] = v;
    }
    pos.current = next;
    setSides(next);
    if (!settled) raf.current = requestAnimationFrame(tick);
  }, []);

  const startLoop = useCallback(() => {
    if (raf.current) return;
    lastT.current = performance.now();
    raf.current = requestAnimationFrame(tick);
  }, [tick]);

  useEffect(
    () => () => {
      if (raf.current) cancelAnimationFrame(raf.current);
      document.body.style.cursor = "";
    },
    [],
  );

  const onGripDown = (grip: Grip) => (e: ReactPointerEvent<HTMLDivElement>) => {
    if (drag.current !== null || !rootRef.current) return;
    if (e.pointerType === "mouse" && e.button !== 0) return;
    e.preventDefault();
    // The root is never transformed, so this is the resting size even
    // if a release animation is still in flight.
    const rect = rootRef.current.getBoundingClientRect();
    base.current = { w: Math.max(rect.width, 1), h: Math.max(rect.height, 1) };
    drag.current = {
      id: e.pointerId,
      x: e.clientX,
      y: e.clientY,
      sides: grip.sides,
      instant: window.matchMedia("(prefers-reduced-motion: reduce)").matches,
    };
    try {
      e.currentTarget.setPointerCapture(e.pointerId);
    } catch {
      /* pointer already gone */
    }
    document.body.style.cursor = grip.cursor;
    if (!drag.current.instant) startLoop();
  };

  const onGripMove = (e: ReactPointerEvent<HTMLDivElement>) => {
    const d = drag.current;
    if (!d || e.pointerId !== d.id || !base.current) return;
    const dx = e.clientX - d.x;
    const dy = e.clientY - d.y;
    for (const k of d.sides) {
      target.current[k] = k === "left" ? -dx : k === "right" ? dx : k === "top" ? -dy : dy;
    }
    if (d.instant) {
      pos.current = { ...target.current };
      setSides(pos.current);
    }
  };

  const endDrag = (e: ReactPointerEvent<HTMLDivElement>) => {
    const d = drag.current;
    if (!d || e.pointerId !== d.id) return;
    drag.current = null;
    target.current = { ...REST };
    document.body.style.cursor = "";
    if (d.instant) {
      pos.current = { ...REST };
      vel.current = { ...REST };
      setSides(REST);
    }
  };

  const atRest = sides.top === 0 && sides.right === 0 && sides.bottom === 0 && sides.left === 0;
  const bw = base.current?.w ?? 1;
  const bh = base.current?.h ?? 1;
  // Unclamped on purpose: pull a side through its opposite and the
  // scale goes negative — the text mirror-flips instead of stopping.
  const sx = (bw + sides.left + sides.right) / bw;
  const sy = (bh + sides.top + sides.bottom) / bh;
  const tx = (sides.right - sides.left) / 2;
  const ty = (sides.bottom - sides.top) / 2;
  // The frame must be the NORMALIZED box (CSS collapses a negative-width
  // rect to a sliver), so min/max the edges: when inverted, the border
  // and handles keep hugging the mirrored content.
  const fx0 = Math.min(-sides.left, bw + sides.right);
  const fx1 = Math.max(-sides.left, bw + sides.right);
  const fy0 = Math.min(-sides.top, bh + sides.bottom);
  const fy1 = Math.max(-sides.top, bh + sides.bottom);

  return (
    <div ref={rootRef} className="relative inline-block px-8 py-4 sm:px-12 sm:py-6">
      <div
        className="absolute"
        style={{
          top: fy0,
          right: bw - fx1,
          bottom: bh - fy1,
          left: fx0,
          pointerEvents: "none",
        }}
        aria-hidden
      >
        <svg className="ants absolute inset-0 h-full w-full">
          <rect x="1" y="1" width="calc(100% - 2px)" height="calc(100% - 2px)" rx="2" />
        </svg>
        {GRIPS.map((grip, i) => (
          <div
            key={i}
            style={{
              position: "absolute",
              pointerEvents: "auto",
              touchAction: "none",
              cursor: grip.cursor,
              ...(grip.handle
                ? { width: CORNER, height: CORNER, display: "flex", alignItems: "center", justifyContent: "center" }
                : null),
              ...grip.style,
            }}
            onPointerDown={onGripDown(grip)}
            onPointerMove={onGripMove}
            onPointerUp={endDrag}
            onPointerCancel={endDrag}
            onLostPointerCapture={endDrag}
          >
            {grip.handle && <span className="selection-handle" />}
          </div>
        ))}
      </div>
      <div style={{ transform: atRest ? undefined : `translate(${tx}px, ${ty}px) scale(${sx}, ${sy})` }}>
        {children}
      </div>
    </div>
  );
}
