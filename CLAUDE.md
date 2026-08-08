# etchable — agent notes

Tauri desktop app: live schematic canvas for Zener (.zen) files + embedded
Claude Code session. Cargo workspace in `crates/`, React frontend in `ui/`.

## Commands

- `cargo test --workspace` — all Rust tests (fast; pcb deps are cached)
- `cargo run -p zen-build -- examples/demo/top.zen --pretty` — eval a board,
  dump schematic JSON (the M0 CLI; great for inspecting pipeline output)
- `npm run build` — typecheck (tsc strict) + bundle the frontend
- `npm run tauri dev` — run the app (ALWAYS from the repo root; vite root is
  `ui/`, tauri config in `crates/desktop`)
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
- Webview events: `build-started`, `build-finished` (full `BuildOutput`,
  snake_case), `agent-event` (flat tagged union, camelCase — see
  `crates/desktop/src/agent.rs::flatten`). UI mirrors live in `ui/src/types.ts`;
  keep both sides in sync when touching either.
