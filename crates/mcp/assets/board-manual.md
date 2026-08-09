# Working in etchable

The user sees a live canvas that rebuilds automatically whenever a .zen
file is saved — never run build commands via Bash; the `build` MCP tool
forces a rebuild and returns fresh diagnostics. Use the etchable MCP tools
(`get_board_state`, `get_selection`, `get_schematic`, `get_instance`,
`query_nets`, `get_diagnostics`, `get_parts`, `build`, `check_layout`) to
inspect the design instead of parsing .zen files by hand.

- Instance paths look like `root.SENSE_DIV.R1.R`; a bare refdes like `R1`
  resolves too. This vocabulary is shared with the canvas: what the user
  selects is what `get_selection` returns.
- When the user says "this", "these", or "the selected part", call
  `get_selection`.
- `get_board_state` re-serves this manual along with the live state — call
  it first when starting work or after resuming a session.
- Deep language questions: call `zener_reference` for the authoritative
  Zener guide instead of guessing.

## Rules that differ from what you might expect

Zener looks like Python but is not; the schematic is derived, not drawn.

- `io`/`Net`/`Ground`/`Power`/`Component`/`Module` are prelude names —
  never `load()` them.
- Symbol paths are relative to the .zen file and MUST start with `./` or
  `../` — a bare path is read as a package reference and fails.
- Pins bind by NAME. Never type pin tables from a datasheet;
  `get_symbol_pins` is the ONLY source of pin names and numbers, and
  unmapped pins are hard errors.
- Preserve `# pcb:sch` comment blocks; they hold canvas positions.
- Same source always derives the same schematic — there is no hidden
  canvas state to fix by hand; fix the source instead.

## Projects and part cards

Projects are directories marked by etch.toml. Reusable blocks live in
`components/<name>.zen` with a part card `components/<name>.toml`
(description, mpn, manufacturer, datasheet, `[vendors.lcsc] part = "C…"`);
vendored symbol and footprint files live in `components/<name>.assets/`;
datasheets live at `datasheets/<name>.pdf` and you can Read them directly.

Part selections compose: etch.toml `[parts."<instance-path>"]` overrides
beat component cards, which beat inline mpn/manufacturer attributes —
`get_parts` shows the resolved result with provenance. Keys in etch.toml
are instance paths without the `root.` prefix, never refdes.

## Sourcing parts — follow this order

1. Passives (R/C/L/LED/diode…): stdlib parametric generics via
   `list_library`, ALWAYS with an LCSC part in the component card
   (`[vendors.lcsc] part = "C…"`) — otherwise house-part substitution
   happens silently.
2. Everything else comes from LCSC. `search_parts` queries the live JLCPCB
   assembly catalog (stock, price, Basic/Extended class) alongside local
   libraries, ranked Basic-first. If a class=basic part with stock
   satisfies the requirement, USE IT — every Extended part adds a per-part
   JLC setup fee. Pick Extended only when no Basic part fits, and tell the
   user which requirement forced it. The class is recorded in the card
   (`[vendors.lcsc] basic = true/false`) and `get_parts` summarizes the
   BOM's Basic/Extended split for the user.
3. `get_lcsc_part` BEFORE committing to a part: it shows lifecycle status,
   MSL, price breaks, and whether usable CAD data exists (pin/pad counts
   are the best early warning for a bad EasyEDA part).
4. `add_lcsc_component` is THE way to add a real part: it fetches and
   converts the symbol, footprint, 3D model, and datasheet, vendors
   everything into `components/<name>.assets/`, and writes the card with
   provenance. Converted assets are UNVERIFIED — cross-check pin and pad
   counts against the datasheet, relay every conversion warning to the
   user, and leave provenance.verified alone until a human confirms.
5. `add_component` is an escape hatch for a user-supplied .kicad_sym
   already on disk. Hand-author wrappers only when nothing else fits.

NEVER fetch symbols, footprints, or 3D models via WebFetch or Bash —
`add_lcsc_component` is the only sanctioned pipeline for CAD assets.
Datasheets: `fetch_datasheet` (or add_lcsc_component's built-in), never
curl. jlcpcb.com / lcsc.com WebFetch is for READING product pages only. If
LCSC search is blocked or offline, the tools say so with a retry time —
tell the user and continue with local parts instead of probing.

## Wrapper rules (when writing components by hand)

- `io(Net)` per exposed signal; map EVERY symbol pin in `pins={…}`; tie
  true no-connects to `NotConnected()`.
- `symbol = Symbol(library = "./<name>.assets/<name>.kicad_sym")` — the
  `./` prefix is load-bearing.
- The symbol file is the authority for footprint and part identity when it
  carries them; otherwise set footprint explicitly and give
  `part = Part(mpn=…, manufacturer=…)`, or the board fails the BOM check.
- Every passive gets an explicit mpn plus `[vendors.lcsc]` in its card —
  otherwise house-part substitution happens silently. Verify LCSC
  C-numbers against the value and prefer JLC Basic parts.

## Verify as you go

Work in small bursts — write one or a few components, then `build` and fix
the diagnostics before continuing. When a build has errors the canvas
keeps showing the last good build; nothing you did is visible to the user
until the errors are fixed.

- After wiring nets: `query_nets` (check the critical nets end to end) and
  `query_nets {unconnected: true}`.
- After finishing a module or section: `check_layout` — it is cheap (pure
  geometry, no rendering) and catches what diagnostics can't see:
  overlapping symbols, wires passing through symbol bodies, colliding net
  labels. Scope it to what you just touched and fix problems before
  moving on.
- Before each burst of tool calls, say in one short sentence what you are
  about to do and why.
