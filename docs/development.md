# Developing etchable

Everything you need to hack on etchable. For what the app *is*, start with
the [README](../README.md); for product principles, see
[product.md](product.md); for the visual language, see
[design.md](design.md); for recorded decisions, see [decisions/](decisions/).

## Architecture

```
webview (apps/desktop/src)         rust backend (src-tauri + crates/)
┌─────────────────────┐            ┌──────────────────────────────┐
│ canvas: tscircuit   │◄─ events ──│ desktop  — AppState, watcher,│
│  viewer, id_map sel │            │            commands, fanout  │
│ chat: msgs, tools,  │── invoke ─►│   │            │             │
│  permission prompts │            │   ▼            ▼             │
└─────────────────────┘            │ zen-build   agent-host/proto │
                                   │ eval→checks │ claude subproc │
                                   │ →sch→ERC    │ stream-json    │
                                   │      ▲      │      │         │
                                   │  notify     │      ▼         │
                                   │  watcher    │ mcp (axum)     │
                                   └──────────────────────────────┘
        workspace/*.zen ◄── file edits ── claude ── MCP tools ──┘
```

Three loops:

1. **Watch** — anything writes `.zen` (agent, your editor) → notify → 150 ms
   debounce → rebuild → `build-finished` event → canvas + diagnostics update.
2. **Agent** — user message (+ selection context block) → `claude` subprocess
   stdin → NDJSON events stream back → chat renders text / tool calls /
   inline permission prompts.
3. **Context** — the agent calls MCP tools served by the app itself (wired in
   via a generated `--mcp-config`, zero setup): `build`, `get_diagnostics`,
   `get_schematic(scope, depth)`, `get_circuit_json(scope)`, `get_instance`,
   `query_nets`, `get_selection`.

## Repo layout

- `apps/desktop` — the Tauri app: frontend in `src/`, Rust backend in
  `src-tauri/` (a member of the root cargo workspace).
- `apps/web` — etchable.net landing page + API on Cloudflare Workers,
  deployed by `.github/workflows/deploy-web.yml`.
- `packages/ui` — `@etchable/ui`, the shared design system: theme tokens
  (`theme.css`), the icon source (`icon.svg`), and the
  Button/Input/Chip/Panel/SelectionBox/Cursor components used by both the
  app and the landing page.
- `crates/` — backend libraries (table below).

The macOS release pipeline (`release-app.yml`, Homebrew cask) builds
`apps/desktop`. pnpm is the only package manager; internal packages are
referenced with `file:` specs.

### Crates

| crate | role |
|---|---|
| `zen-build` | The eval pipeline from `pcbc` re-hosted as a library: resolve → eval → electrical checks → schematic → ERC. The **only** crate touching `pcb-*` internals (git-pinned to `v0.4.25`, no semver promise upstream); exports plain serde types only. Also emits the Circuit JSON view-model (`circuit_json.rs`: deterministic ids + `id_map` back to instance paths, layout pass included). Ships an M0 CLI: `cargo run -p zen-build -- board.zen --pretty` (or `--circuit-json`). |
| `agent-proto` | stream-json protocol types + NDJSON codec. Drift-tolerant: unknown events/blocks are preserved as `Unknown`, never dropped. |
| `agent-host` | `claude` subprocess lifecycle: spawn/resume/kill, event broadcast, stdin queue, permission responses, interrupts. |
| `mcp` | Localhost MCP server (streamable HTTP, hand-rolled JSON-RPC on axum). Response-size discipline throughout: scoped, capped, summarized — a context-flooded agent is worse than no tool. |
| `etchable` (`apps/desktop/src-tauri`) | Tauri app: shared state, single builder task, fs watcher, command surface, event fanout. |

## The project format

A project is a directory marked by `etchable.toml` (see
[decisions/0002](decisions/0002-etchable-project-format.md) for rationale):
`etchable.toml` is the single manifest: `[project]` owns the format
version, name, and board entry; `[parts]` owns overrides. Reusable
blocks live in `components/<name>.zen` with an optional part card
`components/<name>.toml` (description, mpn, manufacturer, datasheet,
`[vendors.lcsc] part = "C…"`), and datasheets live at
`datasheets/<name>.pdf`. Part selection resolves per component instance:
`etchable.toml [parts."<path>"]` overrides beat cards beat inline attributes,
with the part-target rule mapping module-addressed selections onto their
unique component descendant. `zen_build::project` implements
load/resolve/scaffold; the app's `open_project`/`create_project` commands
and the `get_parts` MCP tool sit on top. Card and manifest edits refresh
via the watcher without a rebuild (`project-changed` event).

## Building from source

Requirements: Rust (≥ 1.88), Node 20+, pnpm, and the `claude` CLI on `PATH`
(override with `ETCHABLE_CLAUDE_BIN`).

```sh
./scripts/fetch-stdlib.sh   # vendors the Zener stdlib (lib/std) @ v0.4.25
pnpm install
pnpm tauri dev              # from the repo root (or apps/desktop)
```

Then open `examples/demo/board.zen` from the toolbar.

App-local data lives under `~/.etchable/` (decision 0005): `cache/` is
disposable (`rm -rf` safe), `state/etchable.sqlite3` holds recents, agent
sessions, and UI prefs, `runtime/` is per-instance scratch. Deleting
`~/.etchable` fully resets the app.

Env knobs: `ETCHABLE_CLAUDE_BIN` (claude binary path), `ETCHABLE_MODEL`
(model override), `ETCHABLE_OPEN` (absolute path to a .zen board to open at
startup — handy in dev), `ETCHABLE_HOME` (relocate `~/.etchable`),
`ETCHABLE_CACHE_DIR` (relocate just the cache), `ETCHABLE_LCSC_OFFLINE=1`
(no network; CI sets it), `RUST_LOG`.

### M0 pipeline CLI

```sh
cargo run -p zen-build -- examples/demo/board.zen --pretty    # full schematic JSON
cargo run -p zen-build -- examples/demo/board.zen --summary   # {ok, components, nets, ...}
```

## Status vs. plan

- **M0 pipeline spike** — done. Git-dep linking works; decision gate answered:
  positions do *not* come free from `pcb-sch` (only authored `# pcb:sch`
  comments), so `zen-build` owns layout (deterministic Rust pass; authored
  positions win when a board is fully annotated).
- **M1 live diagnostics** — done (watch loop, Problems panel).
- **M2 canvas** — done, rebuilt on tscircuit (PATCH-001): `zen-build` emits
  Circuit JSON + `id_map`, `@tscircuit/schematic-viewer` renders real symbol
  glyphs, net-label flags, module boxes; selection + diagnostic highlighting
  ride per-id CSS. Dropped vs the old canvas: marquee multi-select and
  module-container click targets (see docs/decisions/0001).
- **M3 embedded agent** — done: chat panel, tool activity, inline permissions,
  MCP server with all seven tools.
- **M4 selection as context** — done: selection → `set_selection` →
  `get_selection` MCP tool + structured `<canvas-selection>` block in prompts.
- **M5 polish** — not started (real symbol shapes, module collapse,
  multi-board, session branching, layout handoff).

## Known limits / risks

- `pcb-*` and the stream-json protocol are unstable upstream; both are pinned
  and isolated behind `zen-build` / `agent-proto` respectively. Upgrade
  deliberately.
- Bundled-app distribution needs `lib/std` shipped next to the binary and
  Anthropic auth arrangements for non-personal use; dev-mode use is fine.
- Nets render as net-label flags at each pin (standard schematic idiom), not
  routed wires; `schematic_trace` routing is a possible follow-up.
- The tscircuit npm stack churns fast and has undeclared inter-package deps;
  versions are pinned exact and must move as a set
  (docs/decisions/0001-circuit-json-renderer.md).

## License notes

Links MIT-licensed crates from [diodeinc/pcb](https://github.com/diodeinc/pcb);
`zen-build`'s pipeline re-hosts ~100 lines from `pcbc/src/build.rs` (MIT).
The vendored `lib/std` keeps its upstream license file.
