# 0001 — Circuit JSON view-model + tscircuit viewer (PATCH-001 gates)

Date: 2026-08-08. Status: accepted, implemented in this change.

Zener stays the source of truth and the agent interface; the view-model
becomes Circuit JSON (tscircuit's intermediary format); the renderer becomes
`@tscircuit/schematic-viewer` instead of our own canvas. The instance index
(instance path ↔ schematic node ↔ source span) is unchanged and is the key
space of the new `id_map`.

Note: PATCH-001 says "read first: zen-canvas-plan.md" — no such file exists
in the repo or on this machine; the README's architecture section served as
the prior plan.

## G1 — position source. Chosen branch: **B** (layout pass in zen-build)

Findings, from inspecting `pcb-sch` at the pinned rev (`v0.4.25`, checkout
`c8982e9`) and building `examples/demo/board.zen` plus diodeinc's
`examples/AD7171` through our pipeline:

- `pcb_sch` **computes no positions**. `Schematic.symbol_positions` is
  populated exclusively from authored `# pcb:sch <key> x=… y=… rot=…` source
  comments (`pcb-sch/src/position.rs`); boards without annotations (our demo
  board, 8 of 9 upstream examples) get nothing.
- Authored positions land on the **module instance whose file declares
  them**, keyed `comp:<relative.instance.path>` (plus net-symbol keys like
  `GND`), not on the component instances
  (`pcb-zen-core/src/convert.rs::post_process_all_positions`). Our converter
  took `symbol_positions.iter().next()` — an arbitrary HashMap entry — so
  AD7171's 14 authored positions arrived as *one wrong* position. Fixed in
  `convert.rs`: positions are now distributed onto the addressed component
  instances (sorted keys, first-writer-wins, `@unit` suffixes stripped);
  net-symbol positions are not modelled yet.
- `pcb_sch::hierarchical_layout` is exported but called by nothing in the
  pcb workspace. It is a bare corner-tracking box packer (sizes + hierarchy
  in, bounding boxes out) with no connectivity awareness and no tie-in to
  `Schematic`. Usable as inspiration, not as "free coordinates".
- Branch C (tscircuit layout consuming bare circuit-json): the only engine
  that actually runs standalone is `@tscircuit/schematic-match-adapt`
  0.0.22 — verified working, but its repo is **archived** (read-only since
  2025-08-04) and the package ships **no license**. Maintained tscircuit
  layout is only reachable through `@tscircuit/core`'s React evaluation.
  Rejected.

So: a deterministic layout pass in `zen-build` (Rust) assigns
`schematic_component.center` before emission — bottom-up sizing plus
connectivity-aware per-module packing (directed sibling edges from
output-ish→input-ish pins on local signal nets, longest-path column
layering, two-sweep barycenter row ordering), in `crates/zen-build/src/layout.rs`.
When **every** component in the build carries an authored position, the
authored coordinates win (scaled 1/25.4, y negated for schematic-space y-up;
rotation → symbol orientation variant); partial annotation falls back to
computed layout so the result is always fully positioned and deterministic.
The all-or-nothing rule is **load-bearing for the drag-to-persist edit
loop**: its first save writes every component's position at once, so a
partially-annotated board is never a steady state.

## G2 — viewer interactivity. Chosen branch: **A** (use the viewer directly)

Findings (source-verified against `@tscircuit/schematic-viewer` 2.0.76 dist
+ repo, plus an executed end-to-end test feeding hand-written circuit-json —
no `@tscircuit/core` anywhere):

- The viewer consumes a **plain circuit-json array** (`circuitJson` prop) and
  renders via `circuit-to-svg`; it never imports or evaluates
  `tscircuit`/`@tscircuit/core` at runtime.
- Callbacks that exist: `onSchematicComponentClicked({schematicComponentId})`
  and `onSchematicPortClicked` (ports only when `showSchematicPorts`; note it
  delivers the **source_port_id**). Pan/zoom is built in
  (`use-mouse-matrix-transform`). Read-only unless `editingEnabled`.
- Gaps: no net/trace click callback, no hover callbacks, no diagnostics API.
  All recoverable **without a fork** because the rendered SVG carries stable
  data attributes (`data-schematic-component-id`,
  `data-schematic-net-label-id`, `data-schematic-trace-id`,
  `data-subcircuit-connectivity-map-key`, `data-schematic-port-id`): we
  delegate DOM clicks for net labels and inject per-id CSS (the viewer's
  `css` prop) for selection + diagnostic highlighting.
- Known trap: the viewer memoizes on `circuitJson.length` + `editCount`, not
  identity — rebuilds that keep the element count must bump `editCount`.
- Packaging: several of the stack's runtime deps are **undeclared**, and the
  viewer declares `tscircuit: "*"` as a peer (npm ≥7 would auto-install all
  of core). We pin exact versions, add the undeclared deps explicitly, and
  set `legacy-peer-deps` in `.npmrc`.

What this drops relative to the old canvas (accepted, follow-ups if missed):
marquee multi-select and module-container click targets (modules render as
`schematic_box` outlines, which have no hit targets in the viewer). Shift
+click multi-select on components and net-label click selection are kept.

## Emission scheme (T1)

One module boundary: `crates/zen-build/src/circuit_json.rs`,
`to_circuit_json(&BuildOutput) -> CircuitJsonDoc { elements, id_map }`.
Every id is derived from the instance path / net name and also emitted in an
explicit `id_map: BTreeMap<id, path-or-net>` — nothing parses ids apart.

Per component: `source_component` (ftype from the `type` attribute —
resistor/capacitor/led/diode/inductor map to `simple_*` when their required
numeric value parses from attributes, everything else `simple_chip`),
`schematic_component` (symbol components get `symbol_name` +
`symbol_display_value`; chips get `port_arrangement` by-sides +
`port_labels` keyed by pin-number string + a `schematic_text` refdes,
because the box renderer draws no name itself), and per pin a
`source_port` + `schematic_port`. Per net: `source_net` (is_power /
is_ground from the net kind), one `source_trace` connecting all member
ports (`subcircuit_connectivity_map_key` = net name, which powers the
viewer's built-in net hover), and a `schematic_net_label` at each connected
pin. No `schematic_trace` routing in v1 — connectivity reads via net
labels, matching standard schematic practice. Modules render as
`schematic_box` + `schematic_text` name.

## Pinned versions (exact, `.npmrc` legacy-peer-deps=true)

| package | version | license | why |
|---|---|---|---|
| @tscircuit/schematic-viewer | 2.0.76 | MIT | the renderer |
| circuit-to-svg | 0.0.400 | ISC | viewer's actual SVG engine |
| circuit-json | 0.0.465 | ISC | zod validators for the T2 harness + TS types |
| schematic-symbols | 0.0.238 | MIT | symbol_name validity check + size table source |
| @tscircuit/soup-util | 0.0.41 | ISC | undeclared runtime dep of the viewer |
| @tscircuit/circuit-json-util | 0.0.105 | MIT | undeclared runtime dep of circuit-to-svg |
| circuit-json-to-connectivity-map | 0.0.27 | — | undeclared runtime dep of circuit-to-svg |
| @tscircuit/alphabet | 0.0.25 | — | declared peer of circuit-to-svg |
| zod / format-si-unit | 3.x / latest | MIT | undeclared runtime deps of circuit-json |

The tscircuit ecosystem versions churn fast and undeclared deps mean minor
releases can break installs; exact pins + the lockfile are the guard. Do not
upgrade piecemeal — bump the whole set and re-run the validation harness.

## Validation record (T2)

- `apps/desktop/tools/validate-circuit-json.mjs` checks emitter output against the
  published circuit-json zod schemas + id_map completeness + symbol_name
  validity; wired into `.github/workflows/ci.yml`.
- Smoke check board: **examples/demo/board.zen** (50 elements). Rendered via
  `circuit-to-svg`'s `convertCircuitJsonToSchematicSvg` — the same engine
  circuitjson.com and the viewer embed — and visually inspected: LED glyph,
  box resistors with values, net-label flags on all 8 pins, dashed module
  containers with names.
