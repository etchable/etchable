// A render error used to blank the whole window: React unmounts the tree it
// cannot render, and with no boundary that tree is the app. Boundaries are
// placed per PANEL so a crash in the chat leaves the canvas usable (and vice
// versa), with a root one behind them as the last resort.
//
// The fallback's job is to keep the failure actionable: say which part broke,
// show the actual message, let it be copied somewhere useful, and offer both a
// cheap retry (re-mount the subtree) and the big hammer (reload the window).

import { Component, useEffect, useState, type ErrorInfo, type ReactNode } from "react";
import { Button, IconX } from "@etchable/ui";

type Props = {
  /** What broke, in the user's terms: "The canvas", "The chat". */
  what: string;
  children: ReactNode;
};

type State = {
  error: Error | null;
  componentStack: string | null;
  copied: boolean;
};

export default class ErrorBoundary extends Component<Props, State> {
  state: State = { error: null, componentStack: null, copied: false };

  static getDerivedStateFromError(error: Error): Partial<State> {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    // The console is the only durable record here — keep the stack intact for
    // whoever reads the log, and hold the component stack for the copy button.
    console.error(`[${this.props.what}] render error`, error, info.componentStack);
    this.setState({ componentStack: info.componentStack ?? null });
  }

  private details() {
    const { error, componentStack } = this.state;
    return [
      `${this.props.what} render error`,
      error?.stack ?? String(error),
      componentStack ? `\nComponent stack:${componentStack}` : "",
    ]
      .filter(Boolean)
      .join("\n");
  }

  private copy = () => {
    void navigator.clipboard
      .writeText(this.details())
      .then(() => {
        this.setState({ copied: true });
        setTimeout(() => this.setState({ copied: false }), 1500);
      })
      .catch(() => {});
  };

  /** Re-mount the subtree. Cheap, and often enough when the bad state was
   *  transient (one malformed message, one half-built payload). */
  private retry = () => this.setState({ error: null, componentStack: null });

  render() {
    const { error, copied } = this.state;
    if (!error) return this.props.children;
    return (
      <div
        role="alert"
        data-testid="error-boundary"
        className="flex h-full min-h-0 w-full min-w-0 flex-1 items-center justify-center p-6"
      >
        <div className="mx-auto max-w-md rounded-[14px] bg-white p-4 shadow-island ring-1 ring-ink/10">
          <div className="mb-1 text-[12px] font-semibold text-ink/85">
            {this.props.what} stopped rendering
          </div>
          <p className="mb-2.5 text-[11px] leading-relaxed text-ink/55">
            The rest of the app is still running. Retrying re-mounts just this part;
            reloading restarts the window. Your board file is untouched either way.
          </p>
          <pre className="mb-3 max-h-32 overflow-auto rounded-md bg-elev px-2 py-1.5 font-mono text-[10.5px] whitespace-pre-wrap wrap-anywhere text-alert">
            {error.message || String(error)}
          </pre>
          <div className="flex gap-1.5">
            <Button variant="copper" size="sm" className="flex-1" onClick={this.retry}>
              Try again
            </Button>
            <Button
              variant="ghost"
              size="sm"
              className="flex-1"
              onClick={() => window.location.reload()}
            >
              Reload
            </Button>
            <Button variant="ghost" size="sm" className="flex-1" onClick={this.copy}>
              {copied ? "Copied" : "Copy details"}
            </Button>
          </div>
        </div>
      </div>
    );
  }
}

/** Noise that says nothing about the app's health. */
const IGNORED = [
  // Fired by layout thrash in observers; harmless and unactionable.
  /ResizeObserver loop/i,
];

/**
 * Async failures never reach an error boundary: a rejected promise in an event
 * handler or a throw inside setTimeout unwinds outside React's render, so the
 * boundary above stays happy while the operation silently didn't happen.
 *
 * These do NOT take the window over — the app is usually still fine — so this
 * is a dismissible notice rather than a replacement UI, carrying the same
 * copy/reload affordances as the boundary. Repeats of one message collapse into
 * a count so a loop reports once instead of a thousand times.
 */
export function GlobalErrorNotice() {
  const [seen, setSeen] = useState<{ text: string; detail: string; count: number } | null>(null);
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    const record = (text: string, detail: string) => {
      if (!text || IGNORED.some((re) => re.test(text))) return;
      setSeen((prev) =>
        prev && prev.text === text
          ? { ...prev, count: prev.count + 1 }
          : { text, detail, count: 1 },
      );
    };
    const onError = (e: ErrorEvent) => {
      // Resource-load failures also arrive here with no Error attached; they
      // are about a missing asset, not a broken app.
      if (!e.error && e.target !== window) return;
      record(e.error?.message ?? e.message, e.error?.stack ?? e.message);
    };
    const onRejection = (e: PromiseRejectionEvent) => {
      const reason = e.reason;
      const text = reason instanceof Error ? reason.message : String(reason);
      record(text, reason instanceof Error ? (reason.stack ?? text) : text);
    };
    window.addEventListener("error", onError);
    window.addEventListener("unhandledrejection", onRejection);
    return () => {
      window.removeEventListener("error", onError);
      window.removeEventListener("unhandledrejection", onRejection);
    };
  }, []);

  if (!seen) return null;
  return (
    <div
      role="alert"
      data-testid="global-error"
      className="fixed bottom-4 right-4 z-50 max-w-sm rounded-[14px] bg-white p-3 shadow-island ring-1 ring-alert/25"
    >
      <div className="mb-1 flex items-start gap-2">
        <div className="flex-1 text-[11.5px] font-semibold text-ink/85">
          Something failed in the background
          {seen.count > 1 ? ` (${seen.count}×)` : ""}
        </div>
        <button
          type="button"
          title="Dismiss"
          className="flex flex-none cursor-pointer text-ink/40 hover:text-ink/80"
          onClick={() => setSeen(null)}
        >
          <IconX size={11} />
        </button>
      </div>
      <p className="mb-2 text-[11px] leading-relaxed text-ink/55">
        The app is still running, but that operation did not complete.
      </p>
      <pre className="mb-2.5 max-h-24 overflow-auto rounded-md bg-elev px-2 py-1.5 font-mono text-[10.5px] whitespace-pre-wrap wrap-anywhere text-alert">
        {seen.text}
      </pre>
      <div className="flex gap-1.5">
        <Button
          variant="ghost"
          size="sm"
          className="flex-1"
          onClick={() => {
            void navigator.clipboard
              .writeText(seen.detail)
              .then(() => {
                setCopied(true);
                setTimeout(() => setCopied(false), 1500);
              })
              .catch(() => {});
          }}
        >
          {copied ? "Copied" : "Copy details"}
        </Button>
        <Button
          variant="ghost"
          size="sm"
          className="flex-1"
          onClick={() => window.location.reload()}
        >
          Reload
        </Button>
      </div>
    </div>
  );
}
