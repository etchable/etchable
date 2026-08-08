# etchable design language

## Thesis

**etchable is Figma for circuit boards** — a friendly little PCB design tool. EDA software has always looked like an oscilloscope screensaver: dark, dense, hostile to beginners. etchable takes the opposite position: designing a circuit board should feel like sketching in a design tool, not operating lab equipment. The brand borrows its warmth from the tools people love (Figma, Procreate, Pencil) and its materials from the subject itself — copper, substrate, silkscreen, paper.

The landing page states this in one move: **the page is the canvas.** Paper-white dot grid, and the wordmark sitting *selected* — marching ants, corner handles, a dimension chip — as if etchable were an object in its own editor. Named cursors drift nearby ("you?", "us"): an invitation, not a feature tour.

## Palette

Derived from the app icon (navy substrate, copper traces):

| Token | Hex | Role |
|---|---|---|
| `canvas` | `#FBFAF7` | Page background — warm paper, never pure white |
| `ink` | `#232B3F` | Text and dark UI — the icon's navy substrate |
| `ink-soft` | `#6C7385` | Secondary text |
| `grid` | `#E6E4DD` | Dot grid, hairlines, panel borders |
| `copper` | `#C1783C` | The one loud accent: primary CTAs, the trace motif |
| `copper-deep` | `#9E5F27` | Copper's pressed/shadow state |
| `sky` | `#4D9FFF` | Selection UI only: ants, handles, focus rings — never decoration |
| `leaf` | `#2FBF71` | Success |
| `alert` | `#D64545` | Errors |

Rules: copper is the etch — it appears where the user acts (join, create) and in the trace motif, nowhere else. Sky is reserved for "this is selected/focused," matching every design tool the audience knows. Backgrounds stay paper; navy is ink, not a surface (the icon owns the dark-navy look).

## Typography

| Role | Face | Notes |
|---|---|---|
| Display | **Bricolage Grotesque** 800 | Wordmark and headings only. Chunky, friendly, set tight |
| Body | **Karla** | Warm humanist sans, 400/500/700 |
| Utility | **IBM Plex Mono** | Dimension chips, counts, footer — reads as inspector UI |

Mono is structural, not decorative: it marks text that behaves like tool UI (measurements, counts, metadata). If it isn't data, it isn't mono.

## Signature elements

1. **The selection box** — the hero device. Marching-ants dashed border (animated SVG rect), four corner handles, a sky-blue mono chip (`2 layers × friendly`). Use once per page, on the most important object.
2. **The routed trace** — a copper underline that routes at 45° angles and ends in a via dot, drawn in on load. This is the brand's underline; never use a straight rule for emphasis.
3. **Drifting cursors** — named collaborator cursors ("you?" in copper, "us" in sky) on slow ease loops. Hidden on mobile; they never overlap content.

## Motion

One orchestrated moment (the trace draws itself in ~1s after load), plus two ambient loops (ants march, cursors drift). Everything is CSS, transform/stroke only, and fully disabled under `prefers-reduced-motion`. Adding motion elsewhere needs to clear a high bar: does it teach, confirm, or invite?

## Voice

Plain verbs, sentence case, conversational but specific. Buttons say what they do ("Join the waitlist", "Create account"), success states confirm in the same words ("You're in!"), and errors say what happened and what to do, without apologizing. Circuit vocabulary is welcome when it's real (sketch, route, etch, via, layer) and banned when it's garnish.

- Yes: "Sketch your idea, route your traces, etch what matters."
- No: "Supercharge your workflow with next-gen PCB superpowers."

## Components (as built)

- **Pill input + copper button**: rounded-full, 2px `ink/15` border, sky focus border; button has a hard `copper-deep` drop shadow that compresses on press (tactile, tool-like).
- **Panels** (auth): white, 1px `grid` border, 16px radius, soft shadow. Panels are quiet; they never use copper.
- **Chips**: rounded-full white pills with `grid` borders (auth toggle, session greeting) — the page's "toolbar" register.
- **Focus**: 2px sky outline, 2px offset, on `:focus-visible` globally.

## App chrome (desktop)

Studied from Pencil.app and Figma's UI3 (token-level extraction of both);
adapted to our palette. The governing idea from both: **the canvas is the
star — chrome is small, quiet, and floats on it or recedes beside it.**

- **Surfaces**: the window ground is `chrome` (a warm paper tint one step
  deeper than the canvas); interactive chrome lives on **white islands** —
  small `rounded-lg` cards with a soft shadow *plus a 0.5px hairline ring*
  (Figma's trick: elevation instead of borders, so panels look machined,
  not outlined). Hairline dividers are ink at 6-8% alpha, never solid grey.
- **Text hierarchy is alpha ink**, not grey hexes: primary `ink/92`,
  secondary `ink/55`, faint `ink/35`. Composites correctly on any surface.
- **Type runs tiny**: 11px is the chrome default (12px for chat prose),
  500 weight for labels. Anything that is *data* — paths, values, counts,
  the model name — is mono (unchanged rule: if it isn't data, it isn't
  mono). Icons are Phosphor (phosphoricons.com) via `@etchable/ui`'s
  `Icon*` wrappers — 14px default, regular weight (bold at ≤12px, fill for
  Stop). Never emoji, never unicode glyph characters.
- **Buttons are ghost-first**: borderless, hover = ink wash (`ink/5`).
  The one solid button per surface is copper (our rule beats Pencil's
  monochrome CTA: copper is where the user acts). Controls are ~28px tall.
- **Radii**: 6 / 8 / 10 / 14 — chips and fields at 6-8, islands at 10,
  floating cards at 14. Pills only for status/identity chips.
- **Tabs are a segmented control** (Pencil-style): an `ink/5` well with the
  active segment raised on white + hairline ring.
- **Motion**: 100-150ms ease-out on background/color only. Panels are
  docked, not floating (Figma's UI3 lesson: canvas-first means recessive
  chrome, not floating chrome that crowds small screens).
- **Sky stays selection-only** — also in chrome: focus rings, the selection
  chip, selection context. Never decoration.

## App icon

The wordmark's lowercase "e" — Bricolage Grotesque ExtraBold at display optical size, glyph outline embedded — in black on a clean white squircle with a faint hairline ring. Monochrome on purpose: the icon is typography, and the brand's color belongs to the canvas, not the tile. Source of truth is `packages/ui/icon.svg`; desktop icons (`tauri icon`) and both favicons derive from it. After changing it, `cargo clean -p etchable` so the dev binary re-embeds.
