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

## App icon

Navy squircle, copper PCB traces converging on a chip. The icon is the only place the brand goes dark-navy-dominant; the web canvas stays paper. Favicon and desktop icons derive from the same source image.
