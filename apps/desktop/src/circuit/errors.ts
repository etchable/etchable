// User-voice mapping for writer/gate errors (UX review P0-4). The raw
// strings are precise and correct for the agent, which shares these
// writers; the human door translates them — the product's bet is that
// source is an implementation detail. The raw text is preserved as
// `detail` for a disclosure affordance (toast tooltip).

export type FriendlyError = {
  message: string;
  detail: string;
  /** stale = the board moved under the gesture (retryable); refusal = a
      structural limit (the agent is the escape hatch). */
  kind: "stale" | "refusal" | "other";
};

export function humanizeError(raw: unknown): FriendlyError {
  const detail = String(raw);
  const has = (re: RegExp) => re.test(detail);
  let message = detail;
  let kind: FriendlyError["kind"] = "other";
  if (has(/content modified|board source changed|changed while you were editing/i)) {
    message = "The board changed while you were editing — it re-synced; try again.";
    kind = "stale";
  } else if (has(/changed since this gesture/i)) {
    message = "The file changed since that edit, so its undo history was discarded.";
  } else if (has(/nothing to undo/i)) {
    message = "Nothing to undo.";
  } else if (has(/nothing to redo/i)) {
    message = "Nothing to redo.";
  } else if (
    has(/different module scopes|stays inside .* and isn't exposed|couldn't reach that pin/i)
  ) {
    message =
      "Couldn't reach that pin from the board — its net isn't carried by a port on the " +
      "module's call site. Ask the agent to wire it, or wire it inside the module.";
    kind = "refusal";
  } else if (has(/already unconnected/i)) {
    message = "That pin isn't connected to anything.";
  } else if (has(/already exists|already bound|already defined|already has a child/i)) {
    message = "That name is taken — pick another.";
  } else if (has(/does not parse/i)) {
    message = "The board file has a syntax error — fix it in chat or the editor first.";
  } else if (
    has(/generated|no top-level call|not editable|library source|computed expression|nested scope/i)
  ) {
    message =
      "That part is stamped out by code (a loop or computed name), so the canvas can't " +
      "edit just this copy — ask the agent to change the generating source.";
    kind = "refusal";
  } else if (has(/invalid (instance|net) name/i)) {
    message = "Names start with a letter and use letters, digits, - or _.";
  }
  return { message, detail, kind };
}
