# LCSC/EasyEDA test fixtures

Captured live on 2026-08-08 with `UA="etchable/0.1.1 (+https://github.com/fcjr/etchable)"`.
Tests never touch the network (CI sets `ETCHABLE_LCSC_OFFLINE=1`); these files
are the wire truth the parsers and converters are pinned against.

| File | What it covers | Exact command |
|---|---|---|
| `searchByNumbers.json` | C# -> {uuid, puuid, step} batch mapping | `curl -A "$UA" -X POST https://easyeda.com/api/components/searchByNumbers -H 'Content-Type: application/x-www-form-urlencoded; charset=UTF-8' --data-urlencode 'numbers=["C2040","C25804","C381367"]'` |
| `component_C2040.json` | RP2040, LQFN-56: four-sided pins (60 symbol records), 193 footprint records, SVGNODE with 3D uuid + rotation, nested `packageDetail` | `curl -A "$UA" https://easyeda.com/api/components/c2754a5dac404cb1b757213b56759c67` |
| `component_C25804.json` | 0603 10k resistor: 2-pin passive, `nameAlias=Value`, minimal shapes | `curl -A "$UA" https://easyeda.com/api/components/b210af5a1436310a86e8b108e2e5a90b` |
| `component_C381367.json` | RNP50UAFR100K9: **odd document origin** (~363,310 — nowhere near 4000,3000; the never-hardcode-the-origin counterexample), one ARC record | `curl -A "$UA" https://easyeda.com/api/components/312f3adbace54c75a42b9c7402552ecb` |
| `component_C16214.json` | DC-005 barrel jack: THT OVAL pads with `hole_radius > 0`, slot holes, `cutout` SOLIDREGION | `curl -A "$UA" https://easyeda.com/api/components/163970b9f0738d18a5e45038b79d5fef` |
| `jlc_search_RP2040.json` | JLC search envelope, `componentLibraryType`, price ladder, **lucene highlight markup left intact** (stripping is the parser's job) | `curl -A "$UA" -X POST https://jlcpcb.com/api/overseas-pcb-order/v1/shoppingCart/smtGood/selectSmtComponentList -H 'Content-Type: application/json' -d '{"keyword":"RP2040","currentPage":1,"pageSize":25}'` |
| `jlc_detail_C2040.json` | JLC detail: `componentDesignator` (ref prefix), MSL, assembly mode/process | `curl -A "$UA" 'https://cart.jlcpcb.com/shoppingCart/smtGood/getComponentDetail?componentCode=C2040'` |

Edge cases with no live specimen here — POLYGON pads (rotation baked into
points), `npth` SOLIDREGIONs, `A(1)` pad numbers, whitespace-variant ARC
paths (`M4007.13` vs `M 4002.84`) — are covered by synthetic records in the
converter unit tests; the record grammars come from the decision-0004
research notes.

3D fixtures are tiny synthetic STEP/OBJ files (never a multi-MB blob); the
real C2040 STEP is ~3 MB.
