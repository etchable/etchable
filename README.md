# etchable

A pen.dev-style desktop app for [Zener](https://github.com/diodeinc/pcb): an
infinite-canvas schematic viewer with an embedded Claude Code session, where
`.zen` files in a git repo are the shared substrate between the human and the
agent. You review, select, and prompt on the canvas; the agent edits source;
the canvas re-renders live.

The schematic is *derived* (`schematic = eval(zen)`), so the canvas is
read-only as geometry but writable as **context**: selecting instances on the
canvas resolves to stable paths (`root.SENSE_DIV.R1.R`) that flow into the
agent's prompt and are queryable by the agent over MCP.

## Architecture

```
webview (ui/)                      rust backend (crates/)
┌─────────────────────┐            ┌──────────────────────────────┐
│ canvas: pan/zoom,   │◄─ events ──│ desktop  — AppState, watcher,│
│  hit-test, overlay  │            │            commands, fanout  │
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
   `get_schematic(scope, depth)`, `get_instance`, `query_nets`,
   `get_selection`.

### Crates

| crate | role |
|---|---|
| `zen-build` | The eval pipeline from `pcbc` re-hosted as a library: resolve → eval → electrical checks → schematic → ERC. The **only** crate touching `pcb-*` internals (git-pinned to `v0.4.25`, no semver promise upstream); exports plain serde types only. Ships an M0 CLI: `cargo run -p zen-build -- board.zen --pretty`. |
| `agent-proto` | stream-json protocol types + NDJSON codec. Drift-tolerant: unknown events/blocks are preserved as `Unknown`, never dropped. |
| `agent-host` | `claude` subprocess lifecycle: spawn/resume/kill, event broadcast, stdin queue, permission responses, interrupts. |
| `mcp` | Localhost MCP server (streamable HTTP, hand-rolled JSON-RPC on axum). Response-size discipline throughout: scoped, capped, summarized — a context-flooded agent is worse than no tool. |
| `desktop` | Tauri app: shared state, single builder task, fs watcher, command surface, event fanout. |

## Also in this repo

- `apps/web` — etchable.net landing page + API on Cloudflare Workers,
  deployed by `.github/workflows/deploy-web.yml`.
- `apps/desktop` — the original Tauri scaffold; the macOS release pipeline
  (`release-app.yml`, Homebrew cask) still builds from here. Day-to-day
  development happens in `crates/` + `ui/` at the repo root; consolidating
  the two is pending.

## Setup

Requirements: Rust (≥ 1.88), Node 20+, and the `claude` CLI on `PATH`
(override with `ETCHABLE_CLAUDE_BIN`).

```sh
./scripts/fetch-stdlib.sh   # vendors the Zener stdlib (lib/std) @ v0.4.25
npm install
npm run tauri dev           # run from the repo root
```

Then open `examples/demo/top.zen` from the toolbar. First build of a board
fetches its package deps into `~/.pcb/cache`.

Env knobs: `ETCHABLE_CLAUDE_BIN` (claude binary path), `ETCHABLE_MODEL`
(model override), `ETCHABLE_OPEN` (absolute path to a .zen board to open at
startup — handy in dev), `RUST_LOG`.

### M0 pipeline CLI

```sh
cargo run -p zen-build -- examples/demo/top.zen --pretty    # full schematic JSON
cargo run -p zen-build -- examples/demo/top.zen --summary   # {ok, components, nets, ...}
```

## Status vs. plan

- **M0 pipeline spike** — done. Git-dep linking works; decision gate answered:
  positions do *not* come free from `pcb-sch` (only authored `# pcb:sch`
  comments), so the canvas owns layout (deterministic TS, ELK later if needed).
- **M1 live diagnostics** — done (watch loop, Problems panel).
- **M2 canvas v0** — done: boxes-and-nets, pan/zoom, hit-test, marquee,
  diagnostic badges, net highlighting.
- **M3 embedded agent** — done: chat panel, tool activity, inline permissions,
  MCP server with all six tools.
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
- Nets render as orthogonal trunk routing, not real schematic routing;
  symbol s-expressions from KiCad are available in the build output for a
  future real-symbol renderer.

## License notes

Links MIT-licensed crates from [diodeinc/pcb](https://github.com/diodeinc/pcb);
`zen-build`'s pipeline re-hosts ~100 lines from `pcbc/src/build.rs` (MIT).
The vendored `lib/std` keeps its upstream license file.
