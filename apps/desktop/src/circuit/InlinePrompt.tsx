// A floating single-input card anchored near a canvas point — the shared
// shell for the label-a-pin and rename-a-net gestures. Enter commits
// (errors show inline in user voice, the prompt stays open), Esc closes.
//
// Anatomy follows the app-chrome island recipe (14px radius, soft shadow +
// hairline ring); the commit button is copper — where the user acts — and
// names its object ("Attach to GND"). The card flips away from the anchor
// and clamps to the canvas so it never covers the thing being edited.

import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { Button } from "@etchable/ui";
import { humanizeError } from "./errors";

type InlinePromptProps = {
  /** Dialog purpose, for assistive tech. */
  title: string;
  /** Field label, sentence case ("Net name"). */
  label: string;
  initial: string;
  /** Button verb; the button reads `${verb} ${value}`. */
  verb: string;
  /** Busy text stem; shows `${busyVerb}…`. */
  busyVerb: string;
  /** Client coordinates of the anchoring click. */
  screen: { x: number; y: number };
  wrapRef: React.RefObject<HTMLDivElement | null>;
  onCommit: (value: string) => Promise<void>;
  onClose: () => void;
};

export default function InlinePrompt(props: InlinePromptProps) {
  const { title, label, initial, verb, busyVerb, screen, wrapRef, onCommit, onClose } = props;
  const [value, setValue] = useState(initial);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<ReturnType<typeof humanizeError> | null>(null);
  const [pos, setPos] = useState<{ left: number; top: number } | null>(null);
  const cardRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    inputRef.current?.focus();
    inputRef.current?.select();
  }, []);

  // Flip away from the anchor and clamp to the canvas: prefer below-right
  // of the click; never cover the clicked point itself.
  useLayoutEffect(() => {
    const wrap = wrapRef.current?.getBoundingClientRect();
    const card = cardRef.current?.getBoundingClientRect();
    if (!wrap || !card) return;
    const ax = screen.x - wrap.left;
    const ay = screen.y - wrap.top;
    let left = ax + 16;
    if (left + card.width > wrap.width - 8) left = ax - card.width - 16;
    let top = ay + 16;
    if (top + card.height > wrap.height - 8) top = ay - card.height - 16;
    setPos({
      left: Math.max(8, Math.min(left, wrap.width - card.width - 8)),
      top: Math.max(8, Math.min(top, wrap.height - card.height - 8)),
    });
  }, [screen.x, screen.y, wrapRef, error]);

  // Esc closes the card wherever focus happens to be. It cannot rely on the
  // input's own handler: the input is `disabled` while committing, which blurs
  // it, so after a FAILED commit focus sits on the body and every keystroke
  // missed the card — the prompt was unclosable except by succeeding.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      // Be the only handler: the canvas also listens on window and would
      // clear the selection behind the closing card.
      e.stopPropagation();
      e.stopImmediatePropagation();
      onClose();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [onClose]);

  const commit = async () => {
    const trimmed = value.trim();
    if (!trimmed || busy) return;
    setBusy(true);
    setError(null);
    try {
      await onCommit(trimmed);
    } catch (err) {
      setBusy(false);
      setError(humanizeError(err));
      // Re-enabling the input does not give focus back, so hand it back
      // explicitly: the user's next move is to edit the value or bail out.
      requestAnimationFrame(() => {
        inputRef.current?.focus();
        inputRef.current?.select();
      });
    }
  };

  // Small focus trap: Tab cycles between the input and the button.
  const trapTab = (e: React.KeyboardEvent) => {
    if (e.key !== "Tab") return;
    e.preventDefault();
    const card = cardRef.current;
    if (!card) return;
    const stops = card.querySelectorAll<HTMLElement>("input, button");
    const list = Array.from(stops);
    const at = list.indexOf(document.activeElement as HTMLElement);
    const next = list[(at + (e.shiftKey ? -1 : 1) + list.length) % list.length];
    next?.focus();
  };

  return (
    <div
      ref={cardRef}
      role="dialog"
      aria-label={title}
      className="absolute z-10 w-56 rounded-[14px] bg-white p-2.5 shadow-island ring-1 ring-ink/10"
      style={pos ?? { left: -9999, top: -9999 }}
      onClick={(e) => e.stopPropagation()}
      onKeyDown={trapTab}
    >
      <label className="mb-1 block text-[11px] font-medium text-ink/55">{label}</label>
      <input
        ref={inputRef}
        value={value}
        onChange={(e) => setValue(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") void commit();
          if (e.key === "Escape") {
            e.stopPropagation();
            onClose();
          }
        }}
        disabled={busy}
        className="mb-2 w-full rounded-md border border-ink/15 px-2 py-1 font-mono text-[11.5px] outline-none focus:border-sky/60"
      />
      <Button
        variant="copper"
        size="sm"
        className="w-full"
        disabled={busy || !value.trim()}
        onClick={() => void commit()}
      >
        {busy ? `${busyVerb}…` : `${verb} ${value.trim() || "…"}`}
      </Button>
      {error && (
        <div className="mt-1.5 text-[11px] text-alert" title={error.detail}>
          {error.message}
        </div>
      )}
    </div>
  );
}
