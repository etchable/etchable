// The viewer's camera, mirrored. The viewer stamps its real->svg fit on the
// SVG as data-real-to-screen-transform and applies the user's pan/zoom as a
// CSS matrix on the div wrapping that SVG; the full schematic->screen affine
// is the composition of the two. The dot grid glues its background to it and
// the gesture overlay positions pin targets with it (decision 0009 §7).
//
// Consumers get imperative callbacks, not state — pan/zoom emits a mutation
// per frame.

export type CameraXform = {
  /** x scale (px per schematic unit). */
  a: number;
  /** y scale — negative: schematic y-up, screen y-down. */
  d: number;
  e: number;
  f: number;
};

function parse(raw: string | null | undefined) {
  const m = raw?.match(/matrix\(([^)]*)\)/);
  if (!m) return null;
  const [a, b, c, d, e, f] = m[1].split(/[,\s]+/).map(Number);
  if (![a, b, c, d, e, f].every(Number.isFinite)) return null;
  return { a, d, e, f };
}

export function readCamera(wrap: HTMLElement): CameraXform | null {
  const svg = wrap.querySelector("[data-real-to-screen-transform]");
  const fit = parse(svg?.getAttribute("data-real-to-screen-transform"));
  if (!fit || fit.a === 0) return null;
  // The user pan/zoom matrix lives on an ancestor between the svg and the
  // container; identity when absent (fresh mount, no interaction).
  let user = { a: 1, d: 1, e: 0, f: 0 };
  for (let el = svg?.parentElement; el && el !== wrap; el = el.parentElement) {
    const t = el instanceof HTMLElement ? parse(el.style.transform) : null;
    if (t) {
      user = t;
      break;
    }
  }
  const a = user.a * fit.a;
  if (!Number.isFinite(a) || a === 0) return null;
  return {
    a,
    d: user.d * fit.d,
    e: user.a * fit.e + user.e,
    f: user.d * fit.f + user.f,
  };
}

/** Call `cb` now and on every camera change until the disposer runs. */
export function observeCamera(
  wrap: HTMLElement,
  cb: (cam: CameraXform) => void,
): () => void {
  const sync = () => {
    const cam = readCamera(wrap);
    if (cam) cb(cam);
  };
  sync();
  const observer = new MutationObserver(sync);
  observer.observe(wrap, {
    subtree: true,
    childList: true,
    attributes: true,
    attributeFilter: ["data-real-to-screen-transform", "style"],
  });
  return () => observer.disconnect();
}
