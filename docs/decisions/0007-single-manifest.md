# 0007 — One manifest: etchable.toml, no pcb.toml

Status: accepted, 2026-08-09. Amends 0002 (etch.toml renamed, pcb.toml
dropped from projects). No migration: nothing has shipped, so no project
in the wild carries the old layout.

## Decision

A project has exactly one manifest, `etchable.toml`:

```toml
[project]
version = "0.1"        # manifest format version
name = "my-board"
board = "board.zen"    # optional; falls back to the single root .zen

[parts."SENSE_DIV.R1"] # board-level part overrides, as before
mpn = "RC0402FR-07100KL"
[parts."SENSE_DIV.R1".vendors.lcsc]
part = "C25741"
basic = true
```

`pcb.toml` is no longer written or read for projects. The picker selects
the `etchable.toml` file itself (file dialogs filter by extension only,
so `open_project` validates the basename and still accepts a directory
for pasted paths and recents).

## Why pcb.toml could go

0002 kept `pcb.toml` for two reasons: upstream's parser owns the facts it
models (name, board entry), and byte-compatibility kept `pcb layout` /
`pcb bom` working. What changed in the audit:

- Upstream workspace discovery (`find_workspace_root`) falls back to the
  start directory when no `pcb.toml` exists anywhere up the tree, and
  `get_workspace_info` accepts an absent config (defaults). With the
  frozen dep-less resolver (0005) and the bundled-stdlib patch entry,
  the eval pipeline needs nothing from the file. The demo board builds
  without it (demo_build.rs proves this in CI).
- The name and board entry moved under `[project]` — one file, one
  source of truth, same fallbacks as before (single root .zen; dir name).
- Upstream CLI interop was the real cost. Accepted: etchable is the
  toolchain for etchable projects. If the layout/manufacturing flow ever
  needs upstream's CLI, upstream also supports an inline `# ```pcb`
  manifest block inside the board .zen — a compatible re-entry path that
  still needs no second file.

The vendored stdlib keeps its own `pcb.toml`; that file is upstream's and
load-bearing for stdlib materialization (`materialize_stdlib` checks it).

## Consequences

- The watcher classifies `etchable.toml` as the manifest (workspace
  reopen + project refresh — the board entry can change); all other toml
  stays project-refresh-only.
- One caveat inherited from upstream discovery: a stray `pcb.toml` in an
  ancestor directory of a project would still win root discovery. Not
  guarded against — do not put one there.
- `ETCH_FORMAT_VERSION` is the string "0.1"; a mismatched version is a
  problem entry (tolerant read), never a load failure.
