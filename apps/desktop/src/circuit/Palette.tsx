// The component palette (decision 0009 phases 1–2, reworked per the UX
// review): the left rail beside the canvas. Library and Project tiers are
// offline; Parts searches JLCPCB live. A shared filter narrows the offline
// tiers, arrow keys walk the items, Enter arms, and every install action
// is copper. Symbol glyphs give the list a visual scent — tiny schematic
// pictograms keyed by the generic's shape, not decoration.

import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Button, Spinner } from "@etchable/ui";
import { humanizeError } from "./errors";
import type { LabelArm, LcscSearchHit, PaletteView, PlacementArm } from "../types";

type PaletteProps = {
  /** Refetch signal — bump when the project may have changed. */
  refreshKey: string | null;
  projectOpen: boolean;
  armed: PlacementArm | null;
  onArm: (arm: PlacementArm) => void;
  armedLabel: LabelArm | null;
  onArmLabel: (arm: LabelArm) => void;
};

/** The net-label tools (phase 2): rails first, per the board manual. */
const NET_TOOLS: LabelArm[] = [
  { kind: "Ground", defaultName: "GND", label: "GND" },
  { kind: "Power", defaultName: "VCC_3V3", label: "Power" },
  { kind: "Net", defaultName: "", label: "net label" },
];

/** Scaffold-compatible component name from an MPN. */
function sanitizeName(mpn: string): string {
  let s = mpn.replace(/[^A-Za-z0-9_-]/g, "_");
  if (!/^[A-Za-z]/.test(s)) s = `P${s}`;
  return s.slice(0, 64);
}

/** Tiny schematic pictograms for the offline tiers. Chosen by name; the
    default is a chip outline. Stroke-only, so they read on hover washes. */
function Glyph({ name }: { name: string }) {
  const n = name.toLowerCase();
  let body: React.ReactNode;
  if (n.includes("resistor") || n.includes("ferrite")) {
    body = <path d="M1 8h3M12 8h3M4 5.5h8v5H4z" />;
  } else if (n.includes("capacitor")) {
    body = <path d="M1 8h5M10 8h5M6.5 3.5v9M9.5 3.5v9" />;
  } else if (n.includes("inductor")) {
    body = <path d="M1 9h2a2 2 0 1 1 4 0 2 2 0 1 1 4 0 2 2 0 1 1 4 0" />;
  } else if (n.includes("led")) {
    body = (
      <path d="M1 8h3M12 8h3M4 4l7 4-7 4zM11 4v8M11.5 2.5l2-2M13.5 4.5l2-2" />
    );
  } else if (n.includes("diode") || n.includes("rectifier") || n.includes("zener")) {
    body = <path d="M1 8h3M12 8h3M4 4l7 4-7 4zM11 4v8" />;
  } else if (n.includes("crystal")) {
    body = <path d="M1 8h3M12 8h3M4.5 4.5v7M11.5 4.5v7M6 3.5h4v9H6z" />;
  } else if (n.includes("mosfet") || n.includes("transistor")) {
    body = <path d="M5 3v10M5 5.5h5V2M5 10.5h5V14M1 8h4" />;
  } else if (n.includes("mounting")) {
    body = <path d="M8 3.5a4.5 4.5 0 1 1 0 9 4.5 4.5 0 0 1 0-9zM8 1v14M1 8h14" />;
  } else {
    body = <path d="M4 3.5h8v9H4zM1 6h3M1 10h3M12 6h3M12 10h3" />;
  }
  return (
    <svg
      width="16"
      height="16"
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.3"
      strokeLinecap="round"
      strokeLinejoin="round"
      className="flex-none opacity-60"
      aria-hidden="true"
    >
      {body}
    </svg>
  );
}

type SearchState =
  | { kind: "idle" }
  | { kind: "busy" }
  | { kind: "results"; hits: LcscSearchHit[] }
  | { kind: "error"; message: string };

export default function Palette(props: PaletteProps) {
  const { refreshKey, projectOpen, armed, onArm, armedLabel, onArmLabel } = props;
  const [palette, setPalette] = useState<PaletteView | null>(null);
  const [paletteError, setPaletteError] = useState<string | null>(null);
  const [filter, setFilter] = useState("");
  const [query, setQuery] = useState("");
  const [search, setSearch] = useState<SearchState>({ kind: "idle" });
  const listRef = useRef<HTMLDivElement>(null);
  const filterRef = useRef<HTMLInputElement>(null);
  // The hit whose install form is expanded, plus its editable name.
  const [installing, setInstalling] = useState<{
    hit: LcscSearchHit;
    name: string;
    busy: boolean;
    error: ReturnType<typeof humanizeError> | null;
  } | null>(null);

  const loadPalette = () => {
    setPaletteError(null);
    invoke<PaletteView>("get_palette")
      .then(setPalette)
      .catch((err) => setPaletteError(String(err)));
  };
  useEffect(loadPalette, [refreshKey]);

  // Debounced live search.
  const searchSeq = useRef(0);
  useEffect(() => {
    const q = query.trim();
    if (q.length < 2) {
      setSearch({ kind: "idle" });
      return;
    }
    const seq = ++searchSeq.current;
    setSearch({ kind: "busy" });
    const t = setTimeout(() => {
      invoke<{ status: string; results?: LcscSearchHit[]; hint?: string }>("search_lcsc", {
        query: q,
      })
        .then((r) => {
          if (searchSeq.current !== seq) return;
          if (r.status === "ok") setSearch({ kind: "results", hits: r.results ?? [] });
          else setSearch({ kind: "error", message: r.hint ?? "Search is unavailable." });
        })
        .catch((err) => {
          if (searchSeq.current === seq)
            setSearch({ kind: "error", message: humanizeError(err).message });
        });
    }, 400);
    return () => clearTimeout(t);
  }, [query]);

  const install = async () => {
    if (!installing || installing.busy) return;
    const { hit, name } = installing;
    setInstalling({ ...installing, busy: true, error: null });
    try {
      await invoke("lcsc_install", { name, lcsc: hit.lcsc });
      // The scaffold landed: refresh the local tier and arm placement.
      const p = await invoke<PaletteView>("get_palette");
      setPalette(p);
      setInstalling(null);
      onArm({
        spec: `./components/${name}.zen`,
        label: name,
        prefix: null,
        needsValue: false,
      });
    } catch (err) {
      setInstalling((cur) =>
        cur ? { ...cur, busy: false, error: humanizeError(err) } : cur,
      );
    }
  };

  // `A` focuses the library filter — KiCad's add-symbol reflex. Bound here so
  // the palette owns its own shortcut, and ignored while typing anywhere.
  useEffect(() => {
    const down = (e: KeyboardEvent) => {
      if (e.key !== "a" && e.key !== "A") return;
      if (e.metaKey || e.ctrlKey || e.altKey) return;
      const el = document.activeElement;
      if (
        el instanceof HTMLElement &&
        (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.isContentEditable)
      ) {
        return;
      }
      e.preventDefault();
      filterRef.current?.focus();
      filterRef.current?.select();
    };
    window.addEventListener("keydown", down);
    return () => window.removeEventListener("keydown", down);
  }, []);

  // Arrow keys walk the armable items; Enter clicks the focused one.
  const onListKeyDown = (e: React.KeyboardEvent) => {
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
    e.preventDefault();
    const items = Array.from(
      listRef.current?.querySelectorAll<HTMLButtonElement>("button[data-pal]") ?? [],
    );
    if (items.length === 0) return;
    const at = items.indexOf(document.activeElement as HTMLButtonElement);
    const next =
      items[(at + (e.key === "ArrowDown" ? 1 : -1) + items.length) % items.length];
    next?.focus();
  };

  const item =
    "flex w-full items-center gap-2 truncate rounded-md px-2 py-1 text-left font-mono text-[11px] transition-colors hover:bg-ink/5";
  const heading =
    "px-2 pb-1 pt-3 text-[10px] font-semibold uppercase tracking-wider text-ink/40";

  const match = (s: string) =>
    !filter.trim() || s.toLowerCase().includes(filter.trim().toLowerCase());
  const generics = useMemo(
    () => (palette?.generics ?? []).filter((g) => match(g.name)),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [palette, filter],
  );
  const components = useMemo(
    () => (palette?.components ?? []).filter((c) => match(c.name)),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [palette, filter],
  );

  return (
    <div
      ref={listRef}
      className="flex w-52 flex-none flex-col overflow-y-auto border-r border-ink/10 bg-white/60 pb-3"
      onKeyDown={onListKeyDown}
    >
      <div className="px-2 pt-2">
        <input
          ref={filterRef}
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          placeholder="filter library…"
          aria-label="Filter the library"
          className="w-full rounded-md border border-ink/15 bg-white px-2 py-1 text-[11px] outline-none placeholder:text-ink/30 focus:border-sky/60"
        />
      </div>

      <div className={heading}>Library</div>
      {paletteError && (
        <div className="px-2 py-1">
          <div className="mb-1 text-[11px] text-alert">Couldn't load the library.</div>
          <Button variant="quiet" size="sm" className="w-full" onClick={loadPalette}>
            Retry
          </Button>
        </div>
      )}
      {!paletteError && palette === null && (
        <div className="px-2 text-[11px] text-ink/40">loading…</div>
      )}
      {!paletteError && palette !== null && generics.length === 0 && (
        <div className="px-2 text-[11px] text-ink/40">
          {filter ? "no matches" : "nothing installed"}
        </div>
      )}
      {generics.map((g) => (
        <button
          key={g.spec}
          type="button"
          data-pal
          className={`${item} ${armed?.spec === g.spec ? "bg-sky/10 text-sky" : "text-ink/80"}`}
          title={`${g.name} — click, then click the canvas to place`}
          onClick={() => onArm({ spec: g.spec, label: g.name, prefix: g.prefix, needsValue: g.params.includes("value") })}
        >
          <Glyph name={g.name} />
          <span className="truncate">{g.name}</span>
          {g.prefix && <span className="ml-auto text-ink/35">{g.prefix}</span>}
        </button>
      ))}

      {components.length > 0 && (
        <>
          <div className={heading}>Project</div>
          {components.map((c) => (
            <button
              key={c.spec}
              type="button"
              data-pal
              className={`${item} ${armed?.spec === c.spec ? "bg-sky/10 text-sky" : "text-ink/80"}`}
              title={c.description ?? c.name}
              onClick={() =>
                onArm({ spec: c.spec, label: c.name, prefix: null, needsValue: false })
              }
            >
              <Glyph name={c.name} />
              <span className="truncate">{c.name}</span>
              {c.lcsc && <span className="ml-auto text-ink/35">{c.lcsc}</span>}
            </button>
          ))}
        </>
      )}

      <div className={heading}>Nets</div>
      {NET_TOOLS.map((t) => (
        <button
          key={t.label}
          type="button"
          data-pal
          className={`${item} ${armedLabel?.label === t.label ? "bg-sky/10 text-sky" : "text-ink/80"}`}
          title={`${t.label} — click, then click a pin to attach`}
          onClick={() => onArmLabel(t)}
        >
          <svg
            width="16"
            height="16"
            viewBox="0 0 16 16"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.3"
            strokeLinecap="round"
            className="flex-none opacity-60"
            aria-hidden="true"
          >
            {t.kind === "Ground" ? (
              <path d="M8 2v6M3.5 8h9M5 10.5h6M6.5 13h3" />
            ) : t.kind === "Power" ? (
              <path d="M8 14V6M4.5 6h7L8 1.5z" />
            ) : (
              <path d="M1.5 5h9l3 3-3 3h-9zM13 8h2" />
            )}
          </svg>
          <span className="truncate">{t.label}</span>
          <span className="ml-auto text-ink/35">{t.kind}</span>
        </button>
      ))}

      <div className={heading}>Parts · search JLCPCB</div>
      <div className="px-2">
        <input
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder={projectOpen ? "search parts…" : "open a project to add parts"}
          aria-label="Search JLCPCB parts"
          disabled={!projectOpen}
          className="w-full rounded-md border border-ink/15 bg-white px-2 py-1 font-mono text-[11px] outline-none placeholder:text-ink/30 focus:border-sky/60 disabled:opacity-50"
        />
      </div>
      {search.kind === "busy" && (
        <div className="flex items-center gap-1.5 px-2 py-2 text-[11px] text-ink/40">
          <Spinner /> searching…
        </div>
      )}
      {search.kind === "error" && (
        <div className="px-2 py-2 text-[11px] text-alert">{search.message}</div>
      )}
      {search.kind === "results" && search.hits.length === 0 && (
        <div className="px-2 py-2 text-[11px] text-ink/40">no matches</div>
      )}
      {search.kind === "results" &&
        search.hits.map((h) => {
          const open = installing?.hit.lcsc === h.lcsc;
          return (
            <div key={h.lcsc} className="px-1 py-0.5">
              <button
                type="button"
                data-pal
                className={`${item} ${open ? "bg-sky/10" : ""} !whitespace-normal`}
                title={h.description}
                onClick={() =>
                  setInstalling(
                    open
                      ? null
                      : { hit: h, name: sanitizeName(h.mpn), busy: false, error: null },
                  )
                }
              >
                <span className="block w-full">
                  <span className="block truncate font-bold text-ink/85">{h.mpn}</span>
                  <span className="block truncate text-[10px] text-ink/50">
                    {h.package} · {h.stock > 0 ? `${h.stock} in stock` : "no stock"}
                    {h.unit_price != null && ` · $${h.unit_price.toFixed(3)}`}
                    {" · "}
                    <span className={h.class === "basic" ? "text-leaf-deep" : "text-copper"}>
                      {h.class}
                    </span>
                  </span>
                </span>
              </button>
              {open && installing && (
                <div className="mx-1 mb-1 rounded-[10px] bg-white p-1.5 shadow-island ring-1 ring-ink/10">
                  <label className="mb-0.5 block text-[10px] font-medium text-ink/55">
                    Component name
                  </label>
                  <input
                    value={installing.name}
                    onChange={(e) => setInstalling({ ...installing, name: e.target.value })}
                    onKeyDown={(e) => {
                      if (e.key === "Enter") void install();
                      if (e.key === "Escape") setInstalling(null);
                    }}
                    disabled={installing.busy}
                    className="mb-1.5 w-full rounded-md border border-ink/15 px-1.5 py-0.5 font-mono text-[10.5px] outline-none focus:border-sky/60"
                  />
                  <Button
                    variant="copper"
                    size="sm"
                    className="w-full"
                    disabled={installing.busy}
                    onClick={() => void install()}
                  >
                    {installing.busy ? (
                      <>
                        <Spinner /> Installing {installing.hit.lcsc}…
                      </>
                    ) : (
                      `Install ${installing.hit.lcsc}`
                    )}
                  </Button>
                  {installing.error && (
                    <div className="mt-1 text-[10px] text-alert" title={installing.error.detail}>
                      {installing.error.message}
                    </div>
                  )}
                </div>
              )}
            </div>
          );
        })}
    </div>
  );
}
