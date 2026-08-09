# 0004 — LCSC-native part sourcing and a self-contained app

Status: accepted, 2026-08-08. Amends 0003 (registry tier removed,
`add_component` demoted, packaged-app stdlib limitation fixed).

## Decision

Drop every diode.computer dependency and source real components from
LCSC/JLCPCB directly: search the live assembly catalog, pull symbol +
footprint + 3D model + datasheet, convert to KiCad in-process, and vendor
the results into the project. Stdlib parametric generics remain the path
for passives; the vendored KiCad libraries are hidden from the agent
(`list_library` `include_kicad` defaults to false — generics resolve their
own footprints internally, so nothing is lost). At the same time, the app
becomes self-contained: no runtime PATH lookups, no developer-machine
layout assumptions.

## Why

Decision 0003's own evidence: registry auth never worked across three
projects and eleven days of transcripts, and every successful board in
those sessions was sourced against LCSC part numbers for JLC assembly
anyway. LCSC *is* the ground truth for this workflow. Meanwhile a packaged
`.app` could not discover `lib/std` (the exe-ancestor walk cannot reach it
from `Etchable.app/Contents/MacOS`), so New Project failed for anyone who
installed the real app.

## The pipeline (crates/lcsc)

A new crate with no `pcb-*` dependencies (the zen-build anti-corruption
rule holds by construction). Everything is anonymous — no keys, no login.
Verified live 2026-08-08:

- **Search**: `POST jlcpcb.com/.../selectSmtComponentList` — stock, price
  ladder, datasheet URL, and `componentLibraryType` (Basic vs Extended).
- **Component data**: `searchByNumbers` → `GET /api/components/{uuid}`.
  The `/api/products/{C#}/components` route every community tool uses is
  CloudFront-banned after ~15 requests and the ban outlives backoff; the
  uuid route is byte-identical, batched, unthrottled, and returns the 3D
  model uuid for free. A source-grep test keeps the banned route out.
- **3D**: `modules.easyeda.com/{bucket}/{uuid}`, mirror failover to
  `modules.lceda.cn`, 8 MB cap (over-cap skips with a warning — a missing
  model is inert). The downloadable uuid is the SVGNODE `attrs.uuid`;
  `head.uuid_3d` 404s.

HTTP policy: honest UA (`etchable/x.y (+repo)` — the literal `Mozilla/5.0`
is WAF-blocklisted), serial gate with 250 ms + jitter spacing, one retry
on 429/5xx, and a 30-minute circuit breaker on 403 persisted to disk.
Responses are gzip-sniffed by magic (some paths ignore Accept-Encoding).
`ETCHABLE_LCSC_OFFLINE=1` short-circuits all network (CI sets it).

Cache: `~/Library/Caches/etchable/lcsc/v1` (`ETCHABLE_CACHE_DIR`
override). uuid-addressed docs/models are immutable; `numbers/` 7 d,
`jlc/` 24 h, `search/` 15 min (stock freshness is why search exists —
payloads carry `as_of` + `cached`). Atomic writes, 512 MB LRU sweep.

The **fetch/convert split is load-bearing**: `fetch_part` does I/O and
returns a `RawPart` of unparsed payloads; `convert` is pure. Every
conversion test runs from checked-in JSON fixtures
(`crates/lcsc/tests/fixtures/`, provenance in its README) with golden
`.kicad_sym`/`.kicad_mod` outputs (`UPDATE_GOLDEN=1` regenerates).

## Conversion non-negotiables

Implemented from the wire-format facts and MIT-licensed references only.
**`easyeda2kicad.py` is AGPL-3.0 — never read, port, or cite it**; its
derivatives (`easyeda2kicad-rs`) are treated as contaminated. Permitted:
`easyeda/eext-format-converter` (Apache-2.0), `tscircuit/easyeda-converter`
(MIT), `JLC2KiCad_lib` (MIT), `pcb-jlcpcb` (MIT).

- The symbol's `Footprint` property is EXACTLY the install name — it
  resolves to the `.assets/` sibling; an EasyEDA `C…:NAME` value is a hard
  eval error (the E2E pins this).
- `Manufacturer_Name` + `Manufacturer_Part_Number` are always emitted
  (CJK suffixes stripped) so `symbol_has_identity` holds and codegen never
  needs a `part = Part(…)` splice.
- Pin numbers come from pin-record segment 4 field 4, never
  `spice_pin_number`; an empty pin number is a hard error (pcb-eda would
  silently drop the pin).
- Electrical types are bare symbols; power pins map to `power_in`.
  Duplicate pin names are kept — signal grouping collapses them.
- Geometry: `mm = (ee − origin) × 0.254`, Y negated. Origins come from the
  document (symbol BBox / footprint head) — never hardcoded 4000,3000
  (fixture C381367 sits near 363,310).
- POLYGON pads bake rotation into their points ⇒ emitted at orientation 0.
  `npth` SOLIDREGIONs become NPTH pads; `cutout` is skipped with a warning.
- 3D placement uses the emit-the-offset strategy (geometry untouched —
  baking offsets into a verbatim STEP is a known misplacement bug
  elsewhere), with the outline-centre correction past 0.1 mm.
- Emitters are byte-deterministic: fixed float format, no `(uuid …)`, and
  never `(embedded_files …)` (the one construct the eval validator
  checksums).

## Installation seam (zen-build)

`install_component` takes asset *content* (symbol text, footprint text,
extra assets with name/extension/size validation) and owns validation, the
clobber guard, the single-symbol invariant, codegen + splices, and the
write order (`.assets` → card → `.zen` LAST — the watcher trigger).
`add_component` is now path resolution + delegation. Cards gained
`[provenance]` (source, uuids, fetch time, `verified = false` until a
human checks) and `[assets]` tables, split off in `load_card` before the
shared field parser so LCSC cards load with zero problems.

## MCP surface (14 → 16 tools)

- `search_parts`: local tier (generics + project components; KiCad
  symbol libraries no longer surface) + the LCSC tier with stock, price,
  class, and a ready-made `add_lcsc_component` hint. Results are ranked
  **Basic-first** (in-stock Basic, in-stock Extended, then out-of-stock —
  stable within groups, applied before the cap so Basic options never fall
  off). Blocked/offline map to actionable statuses, never opaque errors;
  stale cache serves as fallback.
- `get_lcsc_part`: the pre-commit check — identity, ref prefix, class,
  stock, price breaks, MSL, lifecycle, attributes, and the EDA-quality
  probe (`has_symbol/has_footprint/has_3d`, pin/pad counts, first pins).
- `add_lcsc_component`: fetch → convert → install → datasheet, returning
  the reviewable diff plus provenance and the UNVERIFIED notice.
- `add_component`: demoted to an escape hatch for user-supplied files.

**Basic-first is policy, and the class is BOM data.** The steering prompt
tells the agent: if a Basic part with stock satisfies the requirement, use
it (every Extended part adds a per-part JLC setup fee); pick Extended only
when no Basic fits, and say which requirement forced it. The chosen class
persists in the card (`[vendors.lcsc] basic = true/false`, JLC detail
authoritative with the EasyEDA `JLCPCB Part Class` fallback) — part of the
project's reviewable text, not tool-call ephemera — and flows through
`resolve_parts` into `get_parts`, whose payload now carries an
`lcsc_classes` summary (`{basic, extended, unclassified}`) so the BOM's
setup-fee exposure is showable to the user at a glance.

The decisive E2E (`crates/mcp/tests/lcsc_e2e.rs`) drives fixture bytes
through convert → install → `load_project` (zero problems) → a full zen
build with every pin bound, plus the stem-inference variant and a negative
proving a mismatched footprint property fails loudly.

## Self-containment

- No external CLI on any runtime path: the `pcb` registry tier is deleted
  (mcp's only subprocess use); scaffold's `git init` uses gix (pure Rust).
  The one sanctioned external binary is the user-installed `claude` CLI;
  ENOENT maps to `SpawnError::CliNotFound` with the install command.
- The stdlib ships inside the bundle (`bundle.resources` → `Resources/
  stdlib`; `beforeBuildCommand` runs `fetch-stdlib.sh` because `lib/std`
  is gitignored). `OpenOptions::stdlib_source` materializes it into
  `<root>/.pcb/stdlib` and injects a `[patch] stdlib` entry so upstream
  skips exe-ancestor discovery — same on-disk layout as today, so
  `stdlib_dir()`, `@stdlib/…` resolution, and the Read grant are
  unchanged. Under `tauri dev` the bundled copy is absent and discovery
  finds the repo checkout.
- Desktop builds open workspaces offline; a failed open earns one online
  retry only when `pcb.toml` actually declares `[dependencies]`.
- TLS is rustls everywhere; conversion is pure Rust; reference data is
  compiled in.

## Rejected

- Injecting `tscircuit/easyeda-converter` Circuit JSON directly: Circuit
  JSON is our *derived* view-model (`circuit_json.rs` is its only
  serializer); injected parts would break determinism, the build, and ERC.
- Spoofed browser UA: measured worse than an honest one.
- Bundling the `claude` CLI: licensing, size, auth, and self-update all
  argue for user-installed.
