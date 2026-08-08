# 0002 — The etchable project format

Date: 2026-08-08. Status: accepted, implemented in this change.

A project is a directory marked by `etch.toml`:

```
my-board/
  etch.toml            # THE etch manifest: marker + etchable-only data
  pcb.toml             # upstream: [workspace] name/pcb-version + [board] name/path
  board.zen            # entry point ([board].path)
  components/          # reusable building blocks
    <name>.zen
    <name>.toml        # part card sidecar (travels with the component)
  datasheets/
    <name>.pdf         # agent-readable reference, keyed by component name
  layout/              # upstream layout artifacts
  .gitignore           # .pcb/       (project is git-init'd at creation)
```

## Why a separate manifest

Upstream parses `pcb.toml` with `#[serde(deny_unknown_fields)]` on both
`PcbToml` and `WorkspaceConfig` — any custom key is a hard parse error that
aborts workspace discovery for the entire build. Extending `pcb.toml` is
therefore impossible without patching upstream. Keeping it byte-compatible
also keeps `pcb layout`, `pcb bom`, and dependency resolution working on
etchable projects for free.

One source of truth per fact: the project **name** and **board entry** live
in `pcb.toml` (`[workspace].name`, `[board].path`) because upstream already
models them; `etch.toml` and the cards own everything upstream cannot hold.
`load_project` reads `pcb.toml` via `pcb_zen_core::PcbToml::from_path` —
the same strict parser the build uses, never a lenient shadow parser that
would open projects the build then hard-fails on.

## Tolerant vs. strict parsing

`etch.toml` and component cards parse tolerantly: unknown keys, unknown
vendors, bad part numbers, and missing card targets become entries in
`ProjectDoc::problems`, never load failures — the GUI-first move on a broken
project is to open it and let the user or agent fix it. `load_project` only
errors when the directory isn't a project at all (no `etch.toml`). This is
the deliberate inverse of `pcb.toml`'s strictness, and the same tolerance
posture as `agent-proto`'s `Unknown` events.

## Part selection

Composable, multi-vendor, resolved per component instance (most specific
wins per field; vendor maps union with per-key override):

1. `etch.toml [parts."<path>"]` — board-level instance overrides.
2. Component card `components/<name>.toml` — matched by the instance's
   defining file (`InstanceDoc.source_file == components/<name>.zen`).
3. Inline zen attributes (`mpn`/`manufacturer` from the stdlib generics).

**The part-target rule:** a selection addressed at an instance applies to
that instance if it is a component, else to its *unique* component
descendant. A card on a multi-component block records a problem and its
part fields are ignored (its `description` still documents the module) —
otherwise one card would silently claim several physical parts.

**Key vocabulary:** files persist root-stripped instance paths
(`SENSE_DIV.R1`), the same convention as `# pcb:sch` position blocks and
`save_positions`. Refdes keys are rejected by design — refdes renumbering
would silently retarget selections, which makes diffs lie. All API and MCP
output emits full `root.`-prefixed paths.

**Vendors:** each vendor table has a validated schema. `lcsc`: `part`
matching `^C\d+$` (required), `basic` bool (optional; JLC basic-vs-extended
matters for assembly cost). Unknown vendors are preserved raw and surfaced
as problems — never dropped — so foreign selections survive round-trips.

## Naming note

Upstream's board-repo convention uses `modules/` for reusable circuits and
`components/Manufacturer/MPN/` for part definitions. etchable deliberately
uses `components/` for its reusable building blocks — friendlier to the
product's audience — and upstream tooling doesn't care about folder names
(only `pcb.toml` paths and `Module()` load paths matter).

## Known limitation

A packaged `.app` cannot discover `lib/std` (the exe-ancestor walk fails
inside the bundle), so New Project in a bundled build cannot evaluate until
stdlib ships as a Tauri resource. Dev builds are unaffected. Follow-up
tracked in docs/development.md known limits.
