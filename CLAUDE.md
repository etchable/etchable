# etchable — agent notes

Tauri desktop app: live schematic canvas for Zener (.zen) files + embedded
Claude Code session. Layout: backend library crates in `crates/`, the Tauri
app (frontend `src/` + backend `src-tauri/`, a root-workspace cargo member)
in `apps/desktop`, the landing page/API in `apps/web`, shared design tokens
and components in `packages/ui` (`@etchable/ui`). pnpm is the only package
manager (workspace deps use `file:` specs).

Product principles (docs/product.md) that bound design decisions: GUI-first
AND agent-first (source code is an implementation detail users shouldn't
need); derivation must be deterministic (same source → same PCB, byte-for-
byte); every change must persist as a reviewable text diff (a git forge is
the trajectory).

## Commands

- `cargo test --workspace` — all Rust tests (fast; pcb deps are cached)
- `cargo run -p zen-build -- examples/demo/board.zen --pretty` — eval a board,
  dump schematic JSON (the M0 CLI; great for inspecting pipeline output)
- `pnpm --filter @etchable/desktop build` — typecheck (tsc strict) + bundle
  the frontend (`pnpm desktop:build` at the root does the same)
- `pnpm tauri dev` — run the app (from the repo root or `apps/desktop`;
  never from anywhere else — the vite dev server is strict on port 1420)
- `./scripts/fetch-stdlib.sh` — vendor `lib/std` (required once per clone;
  zen eval discovers it by walking up from the executable path)

## Hard rules

- **Only `crates/zen-build` may depend on `pcb-*` / `starlark` crates.** It is
  an anti-corruption layer: nothing `pcb_*` crosses its public API. The git
  tag (`v0.4.25`) and the starlark fork rev in Cargo.toml must always match
  the pcb workspace's own pins — upgrade them together or types won't unify.
- **Only `agent-proto` knows the stream-json wire format.** It parses
  tolerantly (unknown events → `Unknown`, never dropped). Protocol facts are
  verified against CLI 2.1.220, notably: `--permission-prompt-tool stdio` is
  hidden from `--help` but required for permission prompts to arrive as
  `can_use_tool` control requests; allow responses must omit `updatedInput`
  unless replacing the input (null is rejected).
- The MCP server (`crates/mcp`) is deliberately hand-rolled JSON-RPC on axum —
  do not introduce an MCP SDK dependency without discussion. Response-size
  discipline is a design invariant: every tool response is scoped/capped.
- `zen-build` creates a fresh `EvalSession` per build on purpose (sessions
  cache parsed sources by path; reuse would serve stale files to the watcher).

## Contracts

- Instance addressing: dotted paths rooted at `root`
  (e.g. `root.SENSE_DIV.R1.R`); refdes (`R1`) resolve via
  `SchematicDoc::by_refdes`. This vocabulary is shared by canvas selection,
  MCP tools, and prompts — don't fork it.
- Webview events: `build-started`, `build-finished` (versioned `BuildView`:
  `{version, source, schematic, diagnostics, circuit_json, id_map,
  source_hash}`, snake_case — bump `BUILD_PAYLOAD_VERSION` in
  `apps/desktop/src-tauri/src/state.rs` AND `apps/desktop/src/types.ts`
  together), `agent-event` (flat tagged union, camelCase — see
  `apps/desktop/src-tauri/src/agent.rs::flatten`). UI mirrors live in
  `apps/desktop/src/types.ts`; keep both sides in sync when touching either.
- Project format (docs/decisions/0002): a project dir is marked by
  `etch.toml`; `pcb.toml` stays byte-compatible with upstream
  (deny_unknown_fields — NEVER add custom keys to it) and owns name +
  `[board]` entry. Part selection: etch.toml `[parts."<path>"]` overrides >
  `components/<name>.toml` cards > inline attrs, with the part-target rule
  (module-addressed selections land on the unique component descendant).
  File keys are root-stripped paths (like `# pcb:sch`); APIs emit
  `root.`-prefixed. etch.toml/card parsing is TOLERANT (problems, never
  failures). `zen_build::project` is the only implementation; watcher
  routes `*.toml`/datasheet changes to a project-only refresh
  (`project-changed` event, no build flash).
- Embedded-agent permissions: sessions spawn with `--allowedTools`
  auto-allowing the app's own MCP server (`mcp__etchable`) plus `Read`,
  `Edit`, and `Write` inside the open workspace root — those never show
  permission cards (the live canvas IS the review loop for edits). Bash
  and anything outside the workspace still prompt. Wired in
  `apps/desktop/src-tauri/src/agent.rs::ensure_session`.
- Drag-to-move persistence: the viewer's edit event triggers the
  `save_positions` command (save-ALL — every component in one write, which
  is what keeps the layout's all-or-nothing authored rule a non-issue),
  guarded by `base_hash` (= `BuildView.source_hash`) optimistic concurrency.
  `zen_build::write_positions` is the ONLY writer of `# pcb:sch` blocks
  (merge semantics; never destroys foreign keys like `sym:`). There is no
  watcher echo-suppression on purpose: the rebuild after a save IS the edit
  loop's confirmation.
- The canvas view-model is Circuit JSON emitted by
  `crates/zen-build/src/circuit_json.rs` — the ONLY module that serializes
  the format (byte-deterministic; every id resolves via `id_map`, never parse
  ids apart). Routing lives in `crates/zen-build/src/route.rs` (pure
  geometry): local signal nets (≤4 ports, small span) become
  `schematic_trace` wires, power/ground and far-flung nets keep per-pin net
  labels. Invariant: each trace's edges form ONE contiguous polyline (the
  renderer ignores `edge.from` mid-chain), so branching nets emit one main
  chain + branch chains joined by junction dots — the validator enforces
  this. Symbol geometry lives in `crates/zen-build/src/symbol_geom.rs`,
  GENERATED from the pinned schematic-symbols package by
  `pnpm --filter @etchable/desktop gen:symbol-geom` (CI runs `--check`);
  emitted glyph ports must reproduce the symbol's native offsets verbatim
  (symbol coords ARE schematic coords, y-up) or the viewer's angle-matcher
  silently drops the glyph. Pin-mapping failures fall back to chip boxes —
  never render a wrong glyph. The only UI module importing tscircuit
  packages is
  `apps/desktop/src/circuit/`. tscircuit npm deps are pinned exact and bumped
  as a set, then re-validated:
  `cargo run -q -p zen-build -- examples/demo/board.zen --circuit-json | pnpm run --silent validate:circuit-json`.
  See docs/decisions/0001-circuit-json-renderer.md.
