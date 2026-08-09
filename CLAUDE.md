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
  zen eval discovers it by walking up from the executable path; packaged
  apps instead bundle it via `bundle.resources` +
  `OpenOptions::stdlib_source`)

## Hard rules

- **Only `crates/zen-build` may depend on `pcb-*` / `starlark` crates.** It is
  an anti-corruption layer: nothing `pcb_*` crosses its public API. The git
  tag (`v0.4.25`) and the starlark fork rev in Cargo.toml must always match
  the pcb workspace's own pins — upgrade them together or types won't unify.
  **`pcb-zen` itself is deliberately NOT a dependency** (decision 0005): its
  sqlite-backed remote-package resolver is replaced by
  `zen-build/src/frozen.rs` (workspace + stdlib only; remote
  `[dependencies]` bail with a clear error). Never re-add pcb-zen — its
  rusqlite would collide with the store's sqlx over the one
  `links = "sqlite3"` slot.
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
- **`easyeda2kicad.py` is AGPL-3.0 — never read, port, or cite it** (a copy
  exists at `~/.local/share/uv/tools/easyeda2kicad/`; it is off-limits, as
  are its derivatives like `easyeda2kicad-rs`). The workspace is MIT.
  Permitted conversion references: `easyeda/eext-format-converter`
  (Apache-2.0), `tscircuit/easyeda-converter` (MIT), `JLC2KiCad_lib` (MIT),
  `pcb-jlcpcb` (MIT). Wire formats/endpoints/field names are facts and free.
- **The app ships everything it needs.** No runtime PATH lookups, no
  external CLIs (gix for git init, rustls for TLS, pure-Rust conversion);
  the single sanctioned external binary is the user-installed `claude` CLI.
  In `crates/lcsc`, never touch `/api/products/{C#}/components` (CloudFront
  hard-bans it; a test greps for it) — the `searchByNumbers` → uuid route is
  the only sanctioned path, with an honest UA (never `Mozilla/5.0`).

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
- Multi-window (vite multi-page + dynamic windows): the static `dashboard`
  window (index.html, welcome screen) plus one dynamic `app-N` window per
  open project (app.html, canvas + chat). Each project window owns a full
  instance — canvas state, builder task, fs watcher, MCP server, agent
  session — registered in `state::Registry` by window label; per-window
  commands resolve through it, and events emit ONLY to their window
  (`EventTarget::webview_window` in Rust + window-scoped `listen` in
  state.ts — a global listen would hear every project's events). Open flows
  funnel through `commands::open_board_file`: dedup by board path (focus
  the existing window, never duplicate watchers/agents on one file);
  teardown on failure and on window Destroyed (lib.rs). The dashboard hides
  instead of closing and returns when the last project window closes;
  closing it as the last visible window exits.
  `capabilities/default.json` windows are `["dashboard", "app-*"]`.
- Project format (docs/decisions/0002, amended by 0007): ONE manifest,
  `etchable.toml` (format version "0.1"): `[project]` owns version, name,
  and the board entry (entry falls back to the single root .zen, name to
  the dir name); `[parts."<path>"]` owns board-level part overrides.
  Projects carry NO pcb.toml — upstream discovery falls back to the
  board's directory and etchable declares no deps, so nothing needs it
  (the vendored stdlib keeps its own pcb.toml: that one is upstream's and
  load-bearing). The picker selects the etchable.toml file itself
  (dialogs filter by extension only; `open_project` validates the name
  and also accepts a directory). Part selection: overrides >
  `components/<name>.toml` cards > inline attrs, with the part-target rule
  (module-addressed selections land on the unique component descendant).
  File keys are root-stripped paths (like `# pcb:sch`); APIs emit
  `root.`-prefixed. etchable.toml/card parsing is TOLERANT (problems,
  never failures). `zen_build::project` is the only implementation;
  watcher routes etchable.toml to workspace-reopen + project refresh
  (the entry can change) and other `*.toml`/datasheet changes to a
  project-only refresh (`project-changed` event, no build flash).
- Agent scaffolding harness (docs/decisions/0003, amended by 0004 and
  0006): the MCP surface is 18 tools; sourcing never needs Bash. Tool
  names are vendor-neutral; vendor-specific arguments are keyed by vendor
  name (`lcsc: "C…"`), mirroring the cards' `[vendors.<name>]` sections —
  a future vendor adds an argument key, never a tool.
  `get_board_state` is the entry point — it fuses live orientation (build
  status, selection, top-level modules) with the working-rules manual
  (`crates/mcp/assets/board-manual.md`, compiled in as `mcp::BOARD_MANUAL`).
  That manual is the SINGLE source of truth for agent working rules: the
  system-prompt suffix in `agent.rs` is preamble + `mcp::BOARD_MANUAL` —
  edit the .md, never fork prose into agent.rs, and keep its tool
  references in sync with tools.rs. `check_layout`
  (`zen_build::check_layout`) is the cheap verify tier: pure-geometry lint
  (symbol overlaps, wires through bodies, net-label collisions) over the
  same layout/route data the canvas renders — no screenshots. Every tool
  def carries MCP annotations (readOnly/destructive/idempotent/openWorld);
  new tools must too (a test enforces it). Real parts come from LCSC
  via `search_parts` (live JLCPCB tier: stock/price/Basic-vs-Extended) →
  `get_part` (pre-commit check incl. EDA-quality probe) →
  `add_component` (with `lcsc`: fetch → convert → `install_component`;
  with `symbol_library`: the user-supplied-file escape hatch);
  `crates/lcsc` owns the client/cache/converters (fetch/convert split:
  conversion is pure and fixture-tested, network failures map to
  actionable statuses — never opaque errors). Basic-first is policy:
  search results rank in-stock Basic parts first, the chosen class
  persists in the card (`[vendors.lcsc] basic = true/false`), and
  `get_bom` (the BOM view — deliberately NOT named get_parts, to keep it
  a letter apart from `get_part`) summarizes the Basic/Extended split via
  `lcsc_classes` — keep that chain intact.
  Installed wrappers emit `Symbol(library="./…")` — the `./` prefix is
  load-bearing (a bare path parses as a package ref); converted symbols
  always carry `Manufacturer_Name`/`Manufacturer_Part_Number` so no
  `part=Part(...)` splice is needed, and the `Footprint` property is exactly
  the install name (an `X:Y` value is a hard eval error). Cards carry
  `[provenance]` (`verified = false` until a human checks) and `[assets]`.
  Thinking blocks/deltas surface in the chat; keep `flatten()` emitting them
  — but note current-gen models (fable-5, opus-4.8, opus-5) REDACT thinking
  on the wire (signature-only blocks, empty-text deltas; verified against
  CLI 2.1.220), so no thinking renders for them by design. flatten() drops
  empty thinking blocks AND empty deltas so redacted turns don't leave
  contentless "Thinking" rows. Older models (haiku-4.5, opus-4.6) still
  stream thinking text and render fine.
- Chat UI: `apps/desktop/src/chat/assistant-ui/` is VENDORED from the
  assistant-ui shadcn registry (r.assistant-ui.com — re-fetch to update),
  adapted to Phosphor icons and slimmed (no attachments/dictation/branching).
  `src/chat/ui/` holds the shadcn-compat primitives it imports; Collapsible
  is Base UI (not Radix) on purpose — the templates style Base UI's
  `data-open`/`data-panel-open` attributes. The shadcn semantic color tokens
  (`--color-background`, `--color-muted`, …) are mapped onto the etchable
  palette in App.css and exist FOR the vendored components — app code keeps
  using the palette directly. `runtime.ts` folds the ChatItem transcript
  into one assistant message per TURN (mergeTurns) so reasoning/tool parts
  group into accordions; permission prompts ride the tool-approval channel.
- Embedded-agent permissions: sessions spawn with `--allowedTools`
  auto-allowing the app's own MCP server (`mcp__etchable`) plus `Read`,
  `Edit`, and `Write` inside the open workspace root — those never show
  permission cards (the live canvas IS the review loop for edits). Bash
  and anything outside the workspace still prompt. Wired in
  `apps/desktop/src-tauri/src/agent.rs::ensure_session`.
- Local storage (docs/decisions/0005): everything outside a project lives
  under `~/.etchable/` (`cache/` disposable, `state/etchable.sqlite3`
  sea-orm + embedded migrations, `runtime/` per-instance mcp-config
  scratch) — `crates/store` is the only authority for these paths and the
  db; the `Registry` opens the Store once at startup and every window
  instance clones it. Store open failure = run without persistence, NEVER
  brick or wipe. Session recording lives in `agent::pump_events` (init
  upserts, result touches; `--resume` forks a new id linked via
  `resumed_from`). Store DTOs generate `apps/desktop/src/generated/` via
  ts-rs (`pnpm gen:store-types`; CI runs `--check`) — regenerate instead
  of hand-editing.
- Drag-to-move persistence: the viewer's edit event triggers the
  `save_positions` command (save-ALL — every component in one write, which
  is what keeps the layout's all-or-nothing authored rule a non-issue),
  guarded by `base_hash` (= `BuildView.source_hash`) optimistic concurrency.
  `zen_build::write_positions` is the ONLY writer of `# pcb:sch` blocks
  (merge semantics; never destroys foreign keys like `sym:`). The agent's
  path to the same layer is the `set_positions` MCP tool: partial
  schematic-space moves (y-up, get_circuit_json units) run through
  `zen_build::merge_positions` (fills every unmoved component from its
  authored or derived spot, ×25.4 / y-flip into `# pcb:sch` space) and the
  same writer, guarded by the `CanvasState.source_hash` staleness token —
  the agent must never text-edit position blocks. There is no watcher
  echo-suppression on purpose: the rebuild after a save IS the edit loop's
  confirmation.
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
