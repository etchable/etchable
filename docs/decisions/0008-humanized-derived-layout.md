# 0008 — Humanized derived layout

Status: accepted, 2026-08-09. Extends 0001's renderer contract; the
emission invariants (deterministic bytes, id_map, contiguous trace
chains) are unchanged.

## Problem

Derived schematics read as netlist dumps: every stdlib generic wrapped
its component in a dashed module box, every passive lay horizontal,
connected pins never shared a waterline (so even routed nets Z-bent), and
most connections rendered as label pairs. Correct, but nothing like what
a human draws.

## Decision

Five changes, all in the derived-layout path (`layout.rs`, `route.rs`,
`convert.rs`); authored `# pcb:sch` positions behave exactly as before:

1. **Pass-through module collapse.** A module whose only drawable child
   is one component (every stdlib generic) hoists the component into its
   parent's packing. Symbols and wires, not boxes.
2. **Rail idioms.** Two-pin passives classify by their nets: Power+Ground
   = decoupler (stacks in a bank beside the flow), Power+signal =
   pull-up (stands above its signal partner), Ground+signal = pull-down
   (hangs below). All vertical, rail pin facing its rail — orientation
   picks the `_up`/`_down` symbol variant so the TwoPin port mapping does
   the flip. A mutually-partnered pull-up/pull-down pair with no flow
   sibling is a voltage divider and fuses into one stacked unit.
3. **Waterline alignment.** After the column/barycenter pass, each unit
   is pushed down (never up, so stacking stays valid) until its
   connecting pin aligns with its predecessor's — series wires run
   straight. Rail attachments and label flags reserve stacking margins so
   nothing collides.
4. **Attachment stubs / partial routing.** A rail passive couples to the
   pin it serves with a short wire even when the net as a whole keeps
   labels (`Layout::stubs` → `RoutedNet::partial`); pins covered by stub
   chains are label-suppressed at emission. This is the "humans wire the
   pull-up to its pin and label the far end" convention.
5. **Rail-kind inference.** Boards that declare rails as bare
   `Net("GND")` still get rail treatment: unmistakable names (GND/VSS
   family, VCC/VDD/VBUS/VIN/V3V3-style) infer Ground/Power at model
   conversion. Presentation-only; typed `Power()`/`Ground()` nets are
   always preferred and unambiguous.

Also fixed en route: `split_crossings` computed edge direction with bare
`f64::signum`, and `signum(0.0) == 1.0` walked the split pieces of
horizontal edges off at 45° — the manhattan invariant now has a
regression test.

Two guarantees added after review of a real board:

- **Chip-edge attachments hang in outside lanes.** A rail passive whose
  chip partner pin sits on the left/right edge cannot x-align under the
  pin (the wire would cross the body); it hangs beside the chip instead,
  offset past the edge's label flags, nested by pin row (lower pins
  closer) so paired attachments draw nested Ls with no crossings. Chip
  boxes also widened (`chip_size`) so the two pin-name columns can't
  collide.
- **Exact label metrics** (the Pencil lesson: never estimate what the
  renderer can tell you). circuit-to-svg sizes net-label flags from its
  own per-glyph table and formula; `gen-text-metrics.mjs` extracts both
  from the PINNED package into the generated `text_metrics.rs`
  (`pnpm gen:text-metrics`, `--check` in CI, same pattern as
  symbol_geom). Label pads, chip-lane clearances, and the layout lint all
  use the exact flag length, and column gaps ADAPT to the sum of facing
  flag extents instead of a fixed constant.
- **No wire ever crosses a component body.** `route_nets` post-validates
  every chain against all body boxes (shrunk by a margin so border-
  touching pin stubs pass; no owner exemption, so a U-turn back across
  the chain's own component also fails) and drops offending nets back to
  labels. A labeled net is honest; a wire through a chip is wrong. Proper
  detour routing around obstacles is the successor to this fallback.

## Consequences

- Layout output changed wholesale; drag-to-move and authored positions
  are unaffected (separate code path, untouched).
- `check_layout` keeps the drawing honest: the demo board must lint clean
  in CI (demo_build.rs).
- Known gaps for later passes: chip power pins still exit left/right
  (top/bottom rails on chips), rail labels are text flags rather than
  GND/VCC glyph symbols, and dense chip pin-name columns can overlap.
