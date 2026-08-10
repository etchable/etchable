<p align="center">
  <img src="apps/desktop/src-tauri/icons/128x128@2x.png" width="128" alt="The etchable icon: copper traces converging on a chip" />
</p>

<h1 align="center">etchable</h1>

<p align="center">
  A friendly little tool for designing circuit boards.<br />
  Sketch your idea, route your traces, etch what matters.
</p>

<p align="center">
  <a href="https://etchable.net">etchable.net</a> ·
  <a href="https://github.com/etchable/etchable/releases">Releases</a> ·
  <a href="docs/development.md">For developers</a>
</p>

---

Describe the board you want and etchable draws you a schematic. Move parts
around, wire pin to pin, and name your power rails right on the canvas — or
ask for a change in chat and watch the drawing follow. Everything you make is
saved as ordinary files on your Mac, so your work stays yours.

- **Start from a sentence.** "A 3.3 V supply with a status LED" is enough to
  get a first schematic on the canvas, ready for you to look over.
- **Draw it yourself.** Place parts from the library, drag them onto the grid,
  wire pin to pin, label ground and power. Undo and redo like any editor.
- **Or just ask.** Click the parts you mean and they become the subject of your
  next message — "make this divider 10k" needs no names or numbers.
- **Real parts you can buy.** Search JLCPCB from the parts panel and see stock,
  price, and whether it's a basic part. Pick one and its symbol and footprint
  come with it.
- **Always live.** Change a file outside the app and the canvas redraws;
  mistakes collect in a Problems panel instead of failing quietly.

## Install

With [Homebrew](https://brew.sh):

```sh
brew install --cask etchable/etchable/etchable
```

Or download the `.dmg` from the [latest release](https://github.com/etchable/etchable/releases/latest).

You'll need:

- A Mac with Apple Silicon, running macOS 13 or newer
- [Claude Code](https://claude.com/claude-code) installed and signed in —
  etchable works through it on your behalf

## Your first board

1. Open etchable and describe the board you want on the welcome screen — or
   open a project you already have.
2. The first build of a board fetches its parts libraries into
   `~/.etchable/cache`. Later builds are fast.
3. Click a part, ask for what you want in the chat, approve the change, and
   watch the canvas update.

A small demo project ships in this repo at `examples/demo` — open that one to
poke around without starting from scratch.

## Early days

etchable is young and moving fast. Wires are drawn between nearby parts while
power and ground use labels at each pin (the usual schematic shorthand);
multi-board projects aren't there yet, and turning a finished schematic into a
physical board layout is still ahead. The rough edges are listed honestly in
[status and limits](docs/development.md#status-vs-plan). Found something
broken? [Open an issue](https://github.com/etchable/etchable/issues).

## License

[MIT](LICENSE)
