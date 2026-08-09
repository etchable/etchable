# 0006 — MCP entry point, layout lint, and the shared board manual

Status: accepted, 2026-08-09. Amends 0003/0004 (MCP surface 16 → 18
tools).

## Decision

Three additions to the agent harness, patterned after a study of Pencil
(the .pen design-canvas app), whose MCP surface demonstrated several
disciplines worth adopting:

1. **`get_board_state` — one orientation call.** Returns the open board,
   project summary, build status with an imperative fix-loop hint, canvas
   selection, top-level modules, and the full working-rules manual. A
   session's first tool call now yields both "where am I" and "how to work
   here" in one round trip.
2. **`check_layout` — the cheap verification tier.** A structural lint
   over the derived drawing (`zen_build::check_layout`): overlapping
   symbol bodies, wires passing through symbols (the router does no
   obstacle avoidance — see route.rs), and net-label collisions. Pure
   geometry over the same `compute_layout` + `route_nets` data the canvas
   renders; no screenshot, no webview. This gives the agent eyes for the
   defect class diagnostics can't express, at near-zero token cost.
3. **MCP tool annotations.** Every tool now declares `title`,
   `readOnlyHint`, `destructiveHint`, `idempotentHint`, `openWorldHint`,
   so clients (including a future tightened permission layer) can tell
   inspection from mutation without name-matching.

## The shared manual

The embedded agent's system-prompt suffix used to be a prose monolith in
`agent.rs`. It is now `crates/mcp/assets/board-manual.md`, compiled in as
`mcp::BOARD_MANUAL` and used from BOTH places: `agent.rs` appends it to
the system prompt (short preamble + manual), and `get_board_state` serves
it to any MCP client. One source of truth; the system prompt and the MCP
surface cannot drift. External clients (a plain `claude` CLI pointed at
the server) now receive the same working rules the embedded agent gets.

## Why (the Pencil study, 2026-08-09)

Pencil's server registers ~10 lean tool schemas and delivers its ~35KB
manual as a *tool result* fused with live editor state, tiers its
verification by token cost (`snapshot_layout` cheap/structural with a
problemsOnly mode, `get_screenshot` explicitly rationed), states its
transactional fix loop in imperative prose ("All operations in this block
have been rolled back. Fix the issues and run batch_design again."), and
annotates every tool. Its write path — a QuickJS DSL (`batch_design`) —
is convergent evidence for our architecture: they had to invent a
programmable mutation layer because their document is a JSON tree; our
document is already a program (.zen), so Read/Edit/Write + watcher
rebuild plays that role natively and needs no new tool.

Adopted: the fused entry point, the cheap verify tier, imperative
fix-loop hints on `build`/`get_diagnostics` responses, and annotations.
Deliberately NOT adopted (for now): a screenshot tool (expensive; revisit
if check_layout proves insufficient for visual-fidelity questions), a
parameterized guide/style library (speculative until we have recurring
EE task-guide content), and spawn-agents parallelism (premature).

## Consequences

- `check_layout` label extents are estimates (the renderer owns font
  metrics), tuned conservative: reported label overlaps are near-certain,
  marginal ones are missed. Constants live in
  `crates/zen-build/src/layout_check.rs`.
- `check_layout` reports wire-through-component for any chain crossing a
  body it doesn't terminate on — including trunks of the wire's own net,
  which is deliberate (it is a visible defect the agent can fix by
  repositioning).
- Tool count assertions and docs must track 18; the manual's tool
  references must stay in sync with `tools.rs` (it is prose, not
  generated).
