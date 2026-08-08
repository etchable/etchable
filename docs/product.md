# etchable product principles

What etchable is for, who it serves, and the three rules every feature has
to clear. The visual language lives in [design.md](design.md); architecture
in [development.md](development.md). This document is about product shape.

## Two first-class users

etchable is built GUI-first and agent-first, at the same time:

- **The human** works on the canvas and in the conversation. They review,
  select, point, and ask. "Make this divider 10k" with a part selected is a
  complete instruction.
- **The agent** works through MCP tools and the source files. It reads the
  schematic the same way the canvas draws it, and edits the same files the
  build derives from.

Both act on one shared substrate — the project's source — and the canvas is
the meeting point: what the human selects becomes the agent's context, and
what the agent edits re-renders under the human's cursor.

**Project code is an implementation detail of the design.** A user should be
able to take a board from idea to fab without reading a line of Zener. The
source is always *there* — inspectable, editable, greppable — and advanced
users will open it, but the product never requires it. The default
expectation: you are either talking to the agent or working in the editor.
Features that assume "the user will just edit the file" are mis-aimed;
features that let the GUI or the agent express the same change are on-target.

## Deterministic derivation

The storage format must always generate the **same** PCB. Same source in,
byte-identical schematic and layout out — every time, on every machine.

There is no ambiguity budget. Anything that influences the result lives in
the source (component definitions, nets, authored `# pcb:sch` positions) or
in a deterministic pass over it (auto-layout, wire routing, symbol
selection). No hidden editor state, no per-machine caches that change
output, no "it depends which order things evaluated."

Why this is non-negotiable: determinism is what makes the other two
principles possible. The canvas can be a pure view of the source only if the
derivation is a function. A diff can be trusted to describe a change only if
nothing outside the diff affects the result. (This is enforced today:
byte-determinism tests on the emitter, sorted iteration everywhere,
authored-positions-win rules with no partial states.)

## Diffable projects

A project is text files in a git repo — nothing else. Every change the GUI
or the agent makes must serialize to a minimal, human-reviewable text
change. Dragging a part on the canvas becomes a `# pcb:sch` comment block in
the board file; a value change is a one-line source edit; there is no
binary sidecar, no database, no project file that merges badly.

This is deliberate groundwork for a **git forge**: boards that get reviewed
like code — pull requests, diffs that mean something, blame that answers
"who moved this and why." A schematic change should read in a diff the way
it reads on the canvas.

The test for any new persistence: *would this diff make sense to a
reviewer?* If a feature's state can't be expressed as clean text in the
repo, the feature needs a different design.

## The bar for new features

Ask three questions, in order:

1. **Does it serve the canvas or the conversation first?** Code-only
   affordances are for advanced use, not the main path.
2. **Does it keep the derivation a pure function?** If it introduces
   ambiguity or hidden state, redesign it.
3. **Does it persist as a reviewable diff?** If a reviewer couldn't read
   the change, neither can the forge we're building toward.
