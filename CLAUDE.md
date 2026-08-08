# etchable — agent notes

Tauri desktop app: live schematic canvas for Zener (.zen) files + embedded
Claude Code session. Layout: backend library crates in `crates/`, the Tauri
app (frontend `src/` + backend `src-tauri/`, a root-workspace cargo member)
in `apps/desktop`, the landing page/API in `apps/web`, shared design tokens
and components in `packages/ui` (`@etchable/ui`). pnpm is the only package
manager (workspace deps use `file:` specs).

## Commands

- `cargo test --workspace` — all Rust tests (fast; pcb deps are cached)
- `cargo run -p zen-build -- examples/demo/top.zen --pretty` — eval a board,
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
  `{version, source, schematic, diagnostics, circuit_json, id_map}`,
  snake_case — bump `BUILD_PAYLOAD_VERSION` in
  `apps/desktop/src-tauri/src/state.rs` AND `apps/desktop/src/types.ts`
  together), `agent-event` (flat tagged union, camelCase — see
  `apps/desktop/src-tauri/src/agent.rs::flatten`). UI mirrors live in
  `apps/desktop/src/types.ts`; keep both sides in sync when touching either.
- The canvas view-model is Circuit JSON emitted by
  `crates/zen-build/src/circuit_json.rs` — the ONLY module that knows the
  format (byte-deterministic; every id resolves via `id_map`, never parse ids
  apart). The only UI module importing tscircuit packages is
  `apps/desktop/src/circuit/`. tscircuit npm deps are pinned exact and bumped
  as a set, then re-validated:
  `cargo run -q -p zen-build -- examples/demo/top.zen --circuit-json | pnpm run --silent validate:circuit-json`.
  See docs/decisions/0001-circuit-json-renderer.md.
