// Provisional parts: a committed placement renders as a stand-in from the
// moment it's written until a schematic-bearing build settles it. With the
// preflight's geometry the stand-in has the REAL outline and REAL pin
// positions, so the swap to the rendered symbol is seamless — and if the
// build goes red, the part stays exactly where it was put: movable,
// pin-clickable, deletable.

import { useEffect, useRef } from "react";
import { observeCamera, readCamera } from "./camera";
import type { GhostGeometry, MoveIn } from "../types";

export type Provisional = {
  /** Top-level instance name (R9). */
  name: string;
  label: string;
  /** io names, for clickable pin stubs. */
  pins: string[];
  x: number;
  y: number;
  rotation: number;
  /** Full component path for position writes (root.R9.R), when known. */
  positionPath: string | null;
  /** Real outline + pin offsets from the preflight, when it evaluated. */
  ghost?: GhostGeometry | null;
};

type ProvisionalLayerProps = {
  parts: Provisional[];
  wrapRef: React.RefObject<HTMLDivElement | null>;
  onPinClick: (
    port: { path: string; pin: string },
    screen: { x: number; y: number },
  ) => void;
  onMove: (moves: Record<string, MoveIn>) => void;
  onMoved: (name: string, x: number, y: number) => void;
};

const FALLBACK_W = 1.4;
const FALLBACK_H = 0.7;

/** Pin offsets from the part center (schematic units, y-up): the real
    geometry when known, else distributed along the fallback box's sides. */
function pinOffsets(p: Provisional): { name: string; x: number; y: number }[] {
  if (p.ghost && p.ghost.pins.length > 0) return p.ghost.pins;
  const perSide = Math.ceil(p.pins.length / 2);
  return p.pins.map((name, i) => {
    const left = i < perSide;
    const idx = left ? i : i - perSide;
    const count = left ? perSide : p.pins.length - perSide;
    const frac = count <= 1 ? 0.5 : idx / (count - 1);
    return {
      name,
      x: (left ? -1 : 1) * (FALLBACK_W / 2),
      y: (0.5 - frac) * FALLBACK_H,
    };
  });
}

export default function ProvisionalLayer(props: ProvisionalLayerProps) {
  const { parts, wrapRef, onPinClick, onMove, onMoved } = props;
  const hostRef = useRef<HTMLDivElement>(null);
  const dragRef = useRef<{
    part: Provisional;
    el: HTMLElement;
    startX: number;
    startY: number;
    moved: boolean;
  } | null>(null);

  // Screen-position every stand-in on camera change (imperative, like the
  // pin overlay). `--sc` is px-per-schematic-unit; all child geometry is
  // calc()'d from it so one variable write per frame does everything.
  useEffect(() => {
    const wrap = wrapRef.current;
    const host = hostRef.current;
    if (!wrap || !host || parts.length === 0) return;
    return observeCamera(wrap, (cam) => {
      const scale = Math.abs(cam.a);
      for (const el of host.querySelectorAll<HTMLElement>("[data-prov]")) {
        const x = Number(el.dataset.x);
        const y = Number(el.dataset.y);
        el.style.transform = `translate(${cam.a * x + cam.e}px, ${cam.d * y + cam.f}px)`;
        el.style.setProperty("--sc", String(scale));
      }
    });
  }, [parts, wrapRef]);

  // Drag-to-move a stand-in: schematic-space delta, committed on mouseup.
  useEffect(() => {
    const move = (e: MouseEvent) => {
      const drag = dragRef.current;
      const wrap = wrapRef.current;
      if (!drag || !wrap) return;
      const cam = readCamera(wrap);
      if (!cam) return;
      drag.moved = true;
      const dx = (e.clientX - drag.startX) / cam.a;
      const dy = (e.clientY - drag.startY) / cam.d;
      const nx = drag.part.x + dx;
      const ny = drag.part.y + dy;
      drag.el.dataset.x = String(nx);
      drag.el.dataset.y = String(ny);
      drag.el.style.transform = `translate(${cam.a * nx + cam.e}px, ${cam.d * ny + cam.f}px)`;
    };
    const up = () => {
      const drag = dragRef.current;
      dragRef.current = null;
      if (!drag?.moved) return;
      const nx = Number(drag.el.dataset.x);
      const ny = Number(drag.el.dataset.y);
      if (drag.part.positionPath) {
        onMove({
          [drag.part.positionPath]: { x: nx, y: ny, rotation: drag.part.rotation },
        });
      }
      onMoved(drag.part.name, nx, ny);
    };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
    return () => {
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
    };
  }, [wrapRef, onMove, onMoved]);

  if (parts.length === 0) return null;

  return (
    <div ref={hostRef} className="pointer-events-none absolute inset-0">
      {parts.map((p) => {
        const w = p.ghost?.width ?? FALLBACK_W;
        const h = p.ghost?.height ?? FALLBACK_H;
        return (
          <div
            key={p.name}
            data-prov
            data-x={p.x}
            data-y={p.y}
            className="absolute left-0 top-0"
          >
            <div
              className="pointer-events-auto absolute cursor-grab rounded-sm border border-dashed border-sky/70 bg-sky/5 active:cursor-grabbing"
              style={{
                width: `calc(var(--sc, 40) * ${w}px)`,
                height: `calc(var(--sc, 40) * ${h}px)`,
                transform: "translate(-50%, -50%)",
              }}
              title={`${p.name} — placed; waiting for the build`}
              onMouseDown={(e) => {
                if (e.button !== 0) return;
                e.stopPropagation();
                const host = (e.currentTarget as HTMLElement).parentElement;
                if (!host) return;
                dragRef.current = {
                  part: p,
                  el: host,
                  startX: e.clientX,
                  startY: e.clientY,
                  moved: false,
                };
              }}
            />
            {pinOffsets(p).map((pin) => (
              <button
                key={pin.name}
                type="button"
                aria-label={`pin ${pin.name} of ${p.name}`}
                title={`${pin.name} — click to attach a net`}
                className="pointer-events-auto absolute h-3 w-3 -translate-x-1/2 -translate-y-1/2 cursor-crosshair rounded-full border border-sky/70 bg-white/80 transition-colors hover:bg-sky/30"
                style={{
                  left: `calc(var(--sc, 40) * ${pin.x}px)`,
                  top: `calc(var(--sc, 40) * ${-pin.y}px)`,
                }}
                onClick={(e) => {
                  e.stopPropagation();
                  onPinClick(
                    { path: `root.${p.name}`, pin: pin.name },
                    { x: e.clientX, y: e.clientY },
                  );
                }}
              />
            ))}
            <div
              className="absolute left-0 -translate-x-1/2 whitespace-nowrap text-[11px] font-medium text-sky/80"
              style={{ top: `calc(var(--sc, 40) * ${h / 2}px + 4px)` }}
            >
              {p.name}
            </div>
          </div>
        );
      })}
    </div>
  );
}
