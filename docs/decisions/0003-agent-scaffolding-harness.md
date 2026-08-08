# 0003 — The agent scaffolding harness

Date: 2026-08-08. Status: accepted, implemented in this change.

> **Amended by 0004 (same day):** the Diode-registry tier of
> `search_parts` is deleted (registry auth never worked; the `pcb` CLI is
> no longer invoked anywhere), `add_component` is demoted to an escape
> hatch for user-supplied files (`add_lcsc_component` is the primary path
> for real parts), and the packaged-app stdlib limitation noted below is
> fixed by bundling the stdlib into the app.

Asked to "create an RP2350 board with USB-C", the embedded agent burned
~15 Bash permission prompts probing `pcb` registry commands (which all fail
without auth) and then curl-ing KiCad's GitLab for symbols — 404s, stale
mirrors, files scattered into /tmp — while showing no reasoning between
tool calls. This record fixes the harness on three axes: capability
(scaffolding tools), visibility (thinking in the chat), and steering (a
playbook that encodes the workflow that actually works).

## Evidence

**Upstream (pinned v0.4.25):** every `pcb search` / `pcb component` path
requires Diode auth — a browser OAuth flow (`pcb auth login`) writing
`~/.pcb/auth.toml`; there is no API-key env and the unauthenticated failure
is swallowed to debug logs. The symbol→`.zen` codegen lives in
`pcb-component-gen` (a 635-line leaf crate), `.kicad_sym` parsing in
`pcb-eda`; both are safe additions at our pin, while `pcb-diode-api` drags
ratatui/rusqlite/sqlite-vec with no feature flags and is deliberately NOT
taken. The vendored stdlib ships 151 single-symbol files and 24 footprint
libraries. Upstream also ships `skills/zener-language/SKILL.md` — the
authoritative language guide.

**Mined successful sessions** (restorekit's dongle-probe: an RP2354A debug
probe taken zero→fab-package in ~2 days; hp-graphics converged on the same
style a week later): registry access NEVER worked across three projects
and eleven days — every successful board was built offline. The winning
pattern, distilled:

1. Inventory local libraries first (stdlib generics + vendored KiCad
   symbols + sibling boards); plan sourcing with zero network assumptions.
2. Extract pins mechanically from `.kicad_sym` — zen binds pins by NAME,
   duplicate names collapse, unmapped pins are hard errors. Never type pin
   tables from a datasheet.
3. Component wrapper anatomy: `io(Net)` per signal; `Component(name,
   symbol=Symbol(library=…), footprint=File(…), pins={…every pin…,
   "NC": NotConnected()})`; bulk no-connects via dict-union comprehension;
   child-relative paths (`//`-rooted paths resolve against the package URL
   and fail); prelude names (`io`, `Net`, …) never `load()`ed.
4. Every passive carries explicit mpn + LCSC selection or the house-part
   matcher substitutes silently. Verify C-numbers against values.
5. Write in bursts, let the compiler grade: build → fix the small set of
   recurring diagnostic classes → clean. Verify through generated
   artifacts (netlists / query tools), not by re-reading source.

## Capability: six MCP tools

All implementations live in `zen-build` (the pcb-* anti-corruption
boundary); the MCP server exposes them with the usual response caps.

- `list_library` — one call inventories stdlib generics (config/io surface
  parsed by line-regex, no eval), vendored KiCad symbol/footprint
  libraries, and the project's own components with cards. Replaces the
  `ls`-and-guess loop.
- `get_symbol_pins` — mechanical pin extraction via `pcb-eda`: names,
  numbers, electrical types, sanitized io names, and the deduped io-group
  map that dictates the `pins={}` keys. Replaces datasheet-typed tables.
- `add_component` — the scaffolding primitive: vendors the symbol (and
  optionally footprint) into the project, generates the wrapper with
  upstream's own codegen (`pcb-component-gen`), and writes the part card.
  Deterministic, offline, path-validated; the full generated text is
  echoed in the tool result so the response is the reviewable diff.
- `search_parts` — tiered: local fuzzy search always works; Diode-registry
  results ride on the `pcb` CLI only when it exists and is authenticated.
  Missing auth is never an error — the payload carries local results plus
  the one-time `pcb auth login` hint to relay to the user.
- `fetch_datasheet` — https-only, size- and type-capped PDF download into
  `datasheets/<component>.pdf`, deduped; kills the curl-for-datasheets
  Bash prompt.
- `zener_reference` — serves the vendored upstream language skill
  (compiled in via `include_str!`, so it works in packaged apps).

## Asset layout: `components/<name>.assets/`

Vendored symbol/footprint files live at
`components/<name>.assets/{<name>.kicad_sym, <name>.kicad_mod}`:

- The component stays one deletable prefix (`<name>.zen`, `<name>.toml`,
  `<name>.assets/`) and copies between projects as a unit — the same
  travels-with-the-component principle as the card (decision 0002).
- The wrapper references `Symbol(library = "<name>.assets/<name>.kicad_sym")`
  — a child-relative path with no `..` and no `//`, immune to the two
  path-resolution failure modes observed in real sessions.
- We copy single-symbol files (the stdlib's own granularity), so a new
  component is a small s-expression text diff, never a monolithic library
  blob. A shared project `lib/` was rejected for coupling components on
  deletion and inviting exactly those blobs.
- `.assets/` directories are invisible to card discovery (which only reads
  top-level `components/*.toml`).

Generated wrappers deliberately omit `Part()`/`properties` — in the
etchable format the part card is the identity layer and `resolve_parts`
composes it. (Known limit: upstream `pcb bom` run outside etchable won't
see card MPNs; follow-up.)

## Visibility: thinking and liveness

Thinking blocks and stream deltas already arrive on the wire and were
being dropped. They now flow through as dimmed, collapsible rows —
streamed live (last few lines visible) and collapsing to a "Thought" pill
once the next item lands. A "Working…" row covers every silent gap
(session spawn, pre-first-token, between tools), pending permission cards
say they are waiting on the user, and `agentRunning` flips optimistically
on send. The playbook also instructs the agent to narrate one sentence
before each tool burst — visibility is a model behavior too, not just a
pipeline fix.

## Steering: playbook and permissions

`SYSTEM_PROMPT_SUFFIX` becomes a scaffolding playbook encoding the
evidence above: sourcing order (list_library → search_parts →
add_component → hand-author with get_symbol_pins as the only pin source),
an explicit prohibition on fetching symbols from the internet or probing
registry commands via Bash, the wrapper rules, the burst-build-verify
cadence, and query_nets verification after wiring.

Permissions widen by exactly two read-only slices: `Read` over the
resolved stdlib dir (reference material the agent could not previously
open without a prompt), and `WebFetch` domain-scoped to jlcpcb.com /
lcsc.com for stock and Basic/Extended checks — the one legitimate network
habit in the mined sessions. Blanket WebFetch stays prompted on purpose:
the permission prompt is the guardrail against the observed
fetch-the-world spiral, and datasheets have a dedicated capped tool.

`mcp__etchable` auto-allow now covers two writing tools
(`add_component`, `fetch_datasheet`). Accepted because both are
path-validated inside the open project, deterministic, and fully echoed
in their results — the canvas and the tool row together are the review
surface, per the product principles.
