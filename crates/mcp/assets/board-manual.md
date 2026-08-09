# Working in etchable

The user sees a live canvas that rebuilds automatically whenever a .zen
file is saved — never run build commands via Bash; the `build` MCP tool
forces a rebuild and returns fresh diagnostics. Use the etchable MCP tools
(`get_board_state`, `get_selection`, `get_schematic`, `get_instance`,
`query_nets`, `get_diagnostics`, `get_bom`, `build`, `check_layout`) to
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
- Preserve `# pcb:sch` comment blocks; they hold canvas positions. Never
  text-edit them — `set_positions` is the structured writer for that
  layer (batch: pass every move in one call; coordinates match
  `get_circuit_json` centers).

## Structured edits — prefer the verbs over raw text edits

The canvas and you share one edit layer: `add_instance`,
`rename_instance`, `set_attribute`, `remove_instance`, `create_net`,
`rename_net`, `connect_pins`, and `disconnect_pin` are the same writers
the user's gestures run. Prefer
them over hand-editing .zen for what they cover — they are serialized
against the user's concurrent gestures, formatting-safe, keep positions
migrated, and keep the user's undo model coherent. Text-edit only what
the verbs can't express (loops, helpers, module authoring).

- `add_instance` places one instance: it ensures the `Module("…")`
  binding, inserts the call in the conventional spot, and (given x/y)
  writes the authored position for the whole board in the same save-all
  write. Pair with `find_empty_space` for a clear spot.
- `rename_instance` renames the `name="…"` literal and migrates
  `# pcb:sch` keys — never rename by text-editing, which orphans
  positions.
- `create_net` defines a net (prefer typed Power/Ground rails);
  `rename_net` renames the variable, the string, and every reference in
  one write, migrating net-symbol positions — a text-edit rename misses
  references and orphans positions.
- `connect_pins` wires two pins (it picks or creates the shared net and
  prunes what that orphans); `disconnect_pin` detaches one. A connect
  that would merge two shared nets returns `needs_merge` — ask the user
  before retrying with `allow_merge`. Endpoints must share one module
  scope; wiring into a submodule goes through its io port.
- `set_attribute` changes value/package/etc. on the creating call;
  `remove_instance` deletes one and prunes what that orphans — confirm
  with the user before removing their work.
- `get_instance` reports `editability` for anything on the board: whether
  a structured edit can target it, why not, and which ancestor (`anchor`)
  an edit must land on. Generated instances (loops, computed names)
  refuse — edit their generating source instead.

## Generative structure

Zener is Starlark: the source language has comprehensions, `for` loops,
and helper functions, and the build evaluates them on every save. For
repeated structure — channel banks, LED arrays, per-rail decoupling,
resistor ladders — write the loop or helper IN the .zen source instead of
unrolling instances by hand. The abstraction persists in the file: one
edit later retunes every channel, and the diff the user reviews says what
changed instead of repeating it N times.

- Give every generated instance a deterministic name derived from the
  loop variable (e.g. `"CH" + str(i)`). Instance paths are identity:
  `# pcb:sch` positions, etchable.toml part overrides, and canvas selection
  all key on them, so unstable names orphan positions and overrides on
  the next rebuild.
- Keep generation deterministic — same source, same schematic. Never
  derive structure from anything outside the source.
- Reach for a helper function once the same sub-circuit appears twice;
  prefer a `Module` when it has a meaningful io boundary.
- Same source always derives the same schematic — there is no hidden
  canvas state to fix by hand; fix the source instead.

## Projects and part cards

Projects are directories marked by etchable.toml (`[project]` holds the
format version, name, and board entry). Reusable blocks live in
`components/<name>.zen` with a part card `components/<name>.toml`
(description, mpn, manufacturer, datasheet, `[vendors.lcsc] part = "C…"`);
vendored symbol and footprint files live in `components/<name>.assets/`;
datasheets live at `datasheets/<name>.pdf` and you can Read them directly.

Part selections compose: etchable.toml `[parts."<instance-path>"]`
overrides beat component cards, which beat inline mpn/manufacturer
attributes — `get_bom` shows the resolved result with provenance. Keys in
etchable.toml are instance paths without the `root.` prefix, never
refdes.

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
   (`[vendors.lcsc] basic = true/false`) and `get_bom` summarizes the
   BOM's Basic/Extended split for the user.
3. `get_part` BEFORE committing to a part: it shows lifecycle status,
   MSL, price breaks, and whether usable CAD data exists (pin/pad counts
   are the best early warning for a bad EasyEDA part). Vendor part
   numbers go under their vendor key (`{lcsc: "C…"}` — lcsc is currently
   the only vendor).
4. `add_component` with `lcsc` is THE way to add a real part: it fetches
   and converts the symbol, footprint, 3D model, and datasheet, vendors
   everything into `components/<name>.assets/`, and writes the card with
   provenance. Converted assets are UNVERIFIED — cross-check pin and pad
   counts against the datasheet, relay every conversion warning to the
   user, and leave provenance.verified alone until a human confirms.
5. `add_component` with `symbol_library` is the escape hatch for a
   user-supplied .kicad_sym already on disk. Hand-author wrappers only
   when nothing else fits.

NEVER fetch symbols, footprints, or 3D models via WebFetch or Bash —
`add_component` is the only sanctioned pipeline for CAD assets.
Datasheets: `fetch_datasheet` (or add_component's built-in), never curl.
jlcpcb.com / lcsc.com WebFetch is for READING product pages only. If
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
- Declare rails with `Power("V3V3")` / `Ground("GND")`, not bare
  `Net(...)` — the canvas draws rail idioms (vertical pull-ups,
  decoupler banks, rail flags) from the net kind, and typed rails also
  strengthen ERC. Conventional names (GND, V3V3, VBUS) are inferred as a
  fallback, but typed is authoritative.

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
  moving on: read the centers from `get_circuit_json`, move components
  with one batched `set_positions` call, then re-run `check_layout`.
- Placing something new (or fixing an overlap): `find_empty_space` returns
  the center of a clear width×height spot beside an anchor (or the whole
  drawing) — pass it straight to `set_positions`. On a hand-arranged
  board, set the position of a newly added component right away so the
  layout stays authored.
- Before each burst of tool calls, say in one short sentence what you are
  about to do and why.
