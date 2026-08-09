# 0009 — Canvas semantic editing via structured source writers

Status: proposed, 2026-08-09. Full requirements in
[../prd-canvas-editing.md](../prd-canvas-editing.md); this records the
decisions that bound it.

## Problem

The canvas is read-mostly: select, and drag-to-move through
`write_positions`. Product principles demand more — a user who never
opens a `.zen` file should be able to place components, wire pins, and
name nets (GUI-first), while the agent collaborates on the same board
(agent-first), and every change persists as a reviewable text diff.

Three designs were considered for how a gesture becomes a change:

- **Overlay sidecar** — gestures write a parallel edit file the build
  merges in. Rejected: a second model that can drift from source, diffs
  that don't read as the change, exactly the "state the rebuild can't
  reproduce" that docs/product.md forbids.
- **Freeform agent edits** — the GUI phrases gestures as prompts and the
  agent text-edits. Rejected as the primary path: non-deterministic,
  slow for direct manipulation, and unusable offline. (It remains the
  escape hatch for anything the writers refuse.)
- **Structured source writers** — each gesture maps to a targeted edit
  of the authored source, exactly as drag-to-move already does through
  `write_positions`. Chosen.

## Decision

1. **Every canvas verb is a structured source writer.** One
   implementation per verb in a new `zen_build::edit` module; two front
   doors — a Tauri command for the canvas and an MCP tool for the agent.
   Neither front door contains edit logic.
2. **Writers are span surgery, not re-printing.** The starlark parser
   (already inside zen-build's sanctioned dependency set) locates
   statement/expression spans; writers replace minimal byte ranges and
   insert at anchored positions. Untouched bytes stay byte-identical, so
   diffs stay minimal and hand-written formatting and comments survive.
3. **An editability map makes refusal honest.** Zener is a real
   language; instances can come from loops and computed expressions.
   Each build statically classifies every instance and net in authored
   files as *literal* (editable: created by a top-level call/assignment
   with a literal name) or *generated* (structurally read-only, with a
   reason). The canvas greys out what it can't edit; writers refuse with
   the reason rather than guess. The map ships in `BuildView` (payload
   version bump) and through MCP.
4. **Connectivity is persisted; wire geometry stays derived.** Drawing a
   wire writes net membership (kwargs + a `Net(...)` definition), never
   route coordinates. The deterministic router draws it — same contract
   as the humanized layout (0008). Authored waypoints are a possible
   later layer, not v1.
5. **All writers share one concurrency gate.** A per-window serialized
   write gate, every write guarded by the build's `source_hash` (the
   existing save_positions token generalized). Stale gestures are
   rejected and re-offered after the rebuild, never silently misapplied.
   The rebuild remains the confirmation — no optimistic canvas state.
6. **Undo is a write-gate snapshot stack.** Per window, per gesture:
   before/after bytes for each touched file; undo restores guarded by
   hash. Entries whose files moved on (the agent wrote) invalidate
   rather than clobber. Git stays the durable trajectory; no auto
   commits.
7. **Interaction is a gesture overlay, not viewer chrome.** The
   full-manual canvas (ghost placement, pin-to-pin wire drag, marquee,
   inline rename) renders in an overlay above @tscircuit/schematic-viewer,
   sharing its camera via the transform-mirroring technique the dot grid
   already uses. The viewer stays a renderer; the existing vendored
   patch surface doesn't grow beyond hit-target needs.

## Consequences

- `BUILD_PAYLOAD_VERSION` bumps (editability map in `BuildView`); UI
  mirrors in `apps/desktop/src/types.ts` follow.
- The MCP surface grows by the edit verbs (~7 tools; annotations
  required as ever); `board-manual.md` gains working rules directing the
  agent to prefer verbs over raw text edits for connectivity and
  renames, so both users converge on the same writers.
- Cross-module wiring is bounded: pins resolve through generic-wrapper
  io to the nearest literal call in an authored file; wiring into a
  submodule's internals from the parent canvas refuses toward the
  module's ports (or the agent).
- A red board stays editable — the canvas must be able to fix a
  failing build. Writers therefore resolve targets by stable name
  (instance `name=`, net variable) against the *current* source at
  apply time, never by build-time spans; unresolvable targets refuse
  per-target, and an unparseable file refuses structured edits toward
  the editor/chat. On a red board this apply-time resolution is the
  whole guard (the `source_hash` token is stale by definition).
- Phased delivery (PRD §9): foundation → add/create components → nets →
  wiring → manipulation completeness → collaboration hardening.
