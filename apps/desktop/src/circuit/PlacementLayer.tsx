// Ghost placement (decision 0009 phase 1): while a palette item is
// armed, a ghost follows the cursor — at the part's REAL size once the
// warm-up returns its geometry — R rotates, Esc ends the session. The
// FIRST drop opens a name/value form (name pre-selected, copper commit);
// every later drop commits immediately with the next free name and the
// carried value, KiCad-style. Commits return straight to aim: the
// provisional stand-in owns the waiting-for-build role, so drops chain
// as fast as the user can click.

import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { Button } from "@etchable/ui";
import { readCamera } from "./camera";
import { humanizeError } from "./errors";
import type { BuildView, PlacementArm } from "../types";

const SNAP = 0.25; // schematic units

type PlacementLayerProps = {
  view: BuildView;
  wrapRef: React.RefObject<HTMLDivElement | null>;
  arm: PlacementArm;
  /** Names taken outside the build's knowledge (provisional parts). */
  takenNames: string[];
  /** Commit the drop; throws with a reason on rejection. */
  onCommit: (
    name: string,
    attrs: [string, string][],
    position: { x: number; y: number; rotation: number },
  ) => Promise<void>;
  /** Placement is over (committed or cancelled). */
  onFinish: () => void;
};

/** Smallest free `${prefix}${n}` against refdes, root child names, and
    names already placed in this arming session (the rebuild lags drops). */
function suggestName(
  view: BuildView,
  prefix: string | null,
  placed: ReadonlySet<string>,
): string {
  const p = prefix ?? "U";
  const taken = new Set<string>(placed);
  const sch = view.schematic;
  if (sch) {
    for (const r of Object.keys(sch.by_refdes)) taken.add(r);
    for (const child of Object.keys(sch.instances["root"]?.children ?? {}))
      taken.add(child);
  }
  for (let n = 1; ; n++) {
    const candidate = `${p}${n}`;
    if (!taken.has(candidate)) return candidate;
  }
}

type Mode =
  | { kind: "aim" }
  | { kind: "form"; pos: { x: number; y: number }; screen: { x: number; y: number } };

export default function PlacementLayer(props: PlacementLayerProps) {
  const { view, wrapRef, arm, takenNames, onCommit, onFinish } = props;
  const takenRef = useRef(takenNames);
  takenRef.current = takenNames;
  const [mode, setMode] = useState<Mode>({ kind: "aim" });
  const [rotation, setRotation] = useState(0);
  // Names committed in this arming session — placement repeats until Esc.
  const placedRef = useRef<Set<string>>(new Set());
  const firstDoneRef = useRef(false);
  const allTaken = () => new Set([...placedRef.current, ...takenRef.current]);
  const [name, setName] = useState(() => suggestName(view, arm.prefix, allTaken()));
  const [value, setValue] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<ReturnType<typeof humanizeError> | null>(null);
  const [formPos, setFormPos] = useState<{ left: number; top: number } | null>(null);
  const ghostRef = useRef<HTMLDivElement>(null);
  const formRef = useRef<HTMLDivElement>(null);
  const nameRef = useRef<HTMLInputElement>(null);

  // The ghost follows the cursor imperatively — no state per mousemove.
  const place = (clientX: number, clientY: number) => {
    const wrap = wrapRef.current;
    const ghost = ghostRef.current;
    if (!wrap || !ghost) return null;
    const cam = readCamera(wrap);
    if (!cam) return null;
    const rect = wrap.getBoundingClientRect();
    const sx = clientX - rect.left;
    const sy = clientY - rect.top;
    // Screen -> schematic, snapped; then back to exact screen so the ghost
    // shows the true drop point.
    const gx = Math.round((sx - cam.e) / cam.a / SNAP) * SNAP;
    const gy = Math.round((sy - cam.f) / cam.d / SNAP) * SNAP;
    moveGhost(gx, gy);
    return { x: gx, y: gy };
  };

  const moveGhost = (gx: number, gy: number) => {
    const wrap = wrapRef.current;
    const ghost = ghostRef.current;
    if (!wrap || !ghost) return;
    const cam = readCamera(wrap);
    if (!cam) return;
    const px = cam.a * gx + cam.e;
    const py = cam.d * gy + cam.f;
    const scale = Math.abs(cam.a);
    ghost.style.display = "block";
    ghost.style.transform = `translate(${px}px, ${py}px) rotate(${-rotation}deg)`;
    const w = arm.ghost?.width ?? 1.4;
    const h = arm.ghost?.height ?? 0.7;
    ghost.style.setProperty("--ghost-w", `${w * scale}px`);
    ghost.style.setProperty("--ghost-h", `${h * scale}px`);
  };

  useEffect(() => {
    const down = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        onFinish();
      } else if (
        mode.kind === "aim" &&
        (e.key === "r" || e.key === "R") &&
        !e.metaKey &&
        !e.ctrlKey &&
        !e.altKey
      ) {
        setRotation((r) => (r + 90) % 360);
      }
    };
    // Capture phase so Escape cancels placement before the canvas's own
    // Escape-clears-selection handler sees it.
    window.addEventListener("keydown", down, true);
    return () => window.removeEventListener("keydown", down, true);
  }, [mode.kind, onFinish]);

  useEffect(() => {
    if (mode.kind === "form") {
      nameRef.current?.focus();
      nameRef.current?.select();
    }
  }, [mode.kind]);

  // Flip/clamp the form so it never covers the drop point or leaves the
  // canvas (island anatomy shared with InlinePrompt).
  useLayoutEffect(() => {
    if (mode.kind !== "form") return;
    const wrap = wrapRef.current?.getBoundingClientRect();
    const card = formRef.current?.getBoundingClientRect();
    if (!wrap || !card) return;
    const ax = mode.screen.x - wrap.left;
    const ay = mode.screen.y - wrap.top;
    let left = ax + 16;
    if (left + card.width > wrap.width - 8) left = ax - card.width - 16;
    let top = ay + 16;
    if (top + card.height > wrap.height - 8) top = ay - card.height - 16;
    setFormPos({
      left: Math.max(8, Math.min(left, wrap.width - card.width - 8)),
      top: Math.max(8, Math.min(top, wrap.height - card.height - 8)),
    });
  }, [mode, wrapRef, error]);

  const attrsFor = (v: string): [string, string][] => (v.trim() ? [["value", v.trim()]] : []);

  const commit = async (commitName: string, pos: { x: number; y: number }) => {
    if (busy) return;
    if (arm.needsValue && !value.trim()) {
      setError({
        message: "A value is required (e.g. 10kohm).",
        detail: "value attr empty",
        kind: "other",
      });
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await onCommit(commitName, attrsFor(value), { x: pos.x, y: pos.y, rotation });
      // Straight back to aim: the provisional stand-in (added by the
      // commit handler) carries the waiting-for-build role.
      setBusy(false);
      firstDoneRef.current = true;
      placedRef.current.add(commitName);
      setName(suggestName(view, arm.prefix, allTaken()));
      setMode({ kind: "aim" });
    } catch (err) {
      setBusy(false);
      setError(humanizeError(err));
      // Recover through the form so the reason is visible even for an
      // immediate repeat drop.
      setMode((m) =>
        m.kind === "form"
          ? m
          : { kind: "form", pos, screen: { x: 0, y: 0 } },
      );
    }
  };

  return (
    <div
      className="absolute inset-0 cursor-crosshair"
      onMouseMove={(e) => {
        if (mode.kind === "aim") place(e.clientX, e.clientY);
      }}
      onClick={(e) => {
        e.stopPropagation();
        if (mode.kind !== "aim") return;
        const pos = place(e.clientX, e.clientY);
        if (!pos) return;
        if (firstDoneRef.current) {
          // Repeat drop: commit immediately with the suggested name and
          // carried value; the form set the pattern on the first drop.
          void commit(name, pos);
        } else {
          setFormPos(null);
          setMode({ kind: "form", pos, screen: { x: e.clientX, y: e.clientY } });
        }
      }}
    >
      {/* The ghost: a dashed outline centered on the (snapped) cursor,
          drawn at the part's real size once the warm-up returns it. */}
      <div
        ref={ghostRef}
        className="pointer-events-none absolute left-0 top-0"
        style={{ display: "none" }}
      >
        <div
          className="rounded-sm border-2 border-dashed border-sky/70 bg-sky/5"
          style={{
            width: "var(--ghost-w)",
            height: "var(--ghost-h)",
            transform: "translate(-50%, -50%)",
          }}
        />
        <div className="absolute left-0 top-0 -translate-x-1/2 translate-y-[calc(var(--ghost-h)/2+4px)] whitespace-nowrap text-[11px] font-medium text-sky">
          {arm.label}
          {mode.kind === "aim" && rotation !== 0 && ` · ${rotation}°`}
        </div>
      </div>

      {mode.kind === "aim" && (
        <div className="pointer-events-none absolute left-1/2 top-3 -translate-x-1/2 rounded-full bg-white px-3.5 py-[5px] text-[11px] font-medium text-ink/70 shadow-island">
          place <span className="font-bold">{arm.label}</span> · R rotates · Esc done
        </div>
      )}

      {mode.kind === "form" && (
        <div
          ref={formRef}
          role="dialog"
          aria-label={`Place ${arm.label}`}
          className="absolute z-10 w-56 rounded-[14px] bg-white p-2.5 shadow-island ring-1 ring-ink/10"
          style={formPos ?? { left: -9999, top: -9999 }}
          onClick={(e) => e.stopPropagation()}
        >
          <label className="mb-1 block text-[11px] font-medium text-ink/55">Name</label>
          <input
            ref={nameRef}
            value={name}
            onChange={(e) => setName(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") void commit(name.trim(), mode.pos);
            }}
            disabled={busy}
            className="mb-2 w-full rounded-md border border-ink/15 px-2 py-1 font-mono text-[11.5px] outline-none focus:border-sky/60"
          />
          {arm.needsValue && (
            <>
              <label className="mb-1 block text-[11px] font-medium text-ink/55">Value</label>
              <input
                value={value}
                onChange={(e) => setValue(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") void commit(name.trim(), mode.pos);
                }}
                placeholder="10kohm"
                disabled={busy}
                className="mb-2 w-full rounded-md border border-ink/15 px-2 py-1 font-mono text-[11.5px] outline-none placeholder:text-ink/30 focus:border-sky/60"
              />
            </>
          )}
          <Button
            variant="copper"
            size="sm"
            className="w-full"
            disabled={busy || !name.trim()}
            onClick={() => void commit(name.trim(), mode.pos)}
          >
            {busy ? `Placing ${name.trim()}…` : `Place ${name.trim() || "…"}`}
          </Button>
          {error && (
            <div className="mt-1.5 text-[11px] text-alert" title={error.detail}>
              {error.message}
            </div>
          )}
        </div>
      )}
    </div>
  );
}
