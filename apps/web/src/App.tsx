import { useEffect, useState, type FormEvent, type ReactNode } from "react";
import { api } from "./lib/api";
import { authClient } from "./lib/auth-client";

function Cursor({
  name,
  color,
  className,
}: {
  name: string;
  color: string;
  className?: string;
}) {
  return (
    <div className={`pointer-events-none absolute ${className ?? ""}`} aria-hidden>
      <svg width="20" height="20" viewBox="0 0 20 20">
        <path
          d="M3 1l5.5 15 2.2-6.3L17 7.5z"
          fill={color}
          stroke="#fff"
          strokeWidth="1.5"
        />
      </svg>
      <span
        className="ml-3 rounded-full px-2 py-0.5 font-mono text-xs text-white"
        style={{ backgroundColor: color }}
      >
        {name}
      </span>
    </div>
  );
}

function SelectionBox({ children }: { children: ReactNode }) {
  const handles = [
    "-top-[5px] -left-[5px]",
    "-top-[5px] -right-[5px]",
    "-bottom-[5px] -left-[5px]",
    "-bottom-[5px] -right-[5px]",
  ];
  return (
    <div className="relative inline-block px-8 py-4 sm:px-12 sm:py-6">
      <svg className="ants absolute inset-0 h-full w-full" aria-hidden>
        <rect x="1" y="1" width="calc(100% - 2px)" height="calc(100% - 2px)" rx="2" />
      </svg>
      {handles.map((pos) => (
        <span key={pos} className={`selection-handle ${pos}`} aria-hidden />
      ))}
      <span
        className="absolute -bottom-3.5 -right-2 rounded bg-sky px-1.5 py-0.5 font-mono text-[11px] leading-tight text-white"
        aria-hidden
      >
        2 layers × friendly
      </span>
      {children}
    </div>
  );
}

function EtchWithTrace() {
  return (
    <span className="relative inline-block">
      etch
      <svg
        className="marker-stroke absolute -bottom-1 left-0 w-full"
        viewBox="0 0 120 12"
        height="12"
        preserveAspectRatio="none"
        aria-hidden
      >
        {/* A routed copper trace ending in a via, like a board layer. */}
        <path
          d="M2 9 L 38 9 L 46 3 L 84 3 L 92 9 L 108 9"
          fill="none"
          stroke="var(--color-copper)"
          strokeWidth="3"
          strokeLinecap="round"
          strokeLinejoin="round"
        />
        <circle cx="113" cy="9" r="3" fill="var(--color-copper)" />
      </svg>
    </span>
  );
}

function WaitlistForm() {
  const [email, setEmail] = useState("");
  const [status, setStatus] = useState<"idle" | "sending" | "done" | "dupe">(
    "idle",
  );
  const [total, setTotal] = useState<number | null>(null);

  useEffect(() => {
    api.api.waitlist.count
      .$get()
      .then((res) => res.json())
      .then(({ count }) => setTotal(count))
      .catch(() => {});
  }, []);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setStatus("sending");
    const res = await api.api.waitlist.$post({ json: { email } });
    if (res.status === 201) {
      setStatus("done");
      setTotal((t) => (t === null ? t : t + 1));
    } else {
      setStatus("dupe");
    }
  };

  if (status === "done" || status === "dupe") {
    return (
      <div className="flex flex-col items-center gap-2">
        <p className="rounded-2xl border-2 border-leaf/30 bg-leaf/10 px-6 py-4 font-medium text-leaf">
          {status === "done"
            ? "You're in! We'll email you when it's ready."
            : "You're already on the list — we haven't forgotten you."}
        </p>
      </div>
    );
  }

  return (
    <div className="flex w-full max-w-md flex-col items-center gap-3">
      <form onSubmit={submit} className="flex w-full gap-2">
        <label htmlFor="waitlist-email" className="sr-only">
          Email address
        </label>
        <input
          id="waitlist-email"
          type="email"
          required
          value={email}
          onChange={(e) => setEmail(e.target.value)}
          placeholder="you@example.com"
          className="min-w-0 flex-1 rounded-full border-2 border-ink/15 bg-white px-5 py-3 text-ink placeholder-ink-soft/60 outline-none transition focus:border-sky"
        />
        <button
          type="submit"
          disabled={status === "sending"}
          className="rounded-full bg-copper px-6 py-3 font-bold text-white shadow-[0_3px_0_var(--color-copper-deep)] transition hover:-translate-y-0.5 hover:shadow-[0_5px_0_var(--color-copper-deep)] active:translate-y-0 active:shadow-[0_2px_0_var(--color-copper-deep)] disabled:opacity-50"
        >
          {status === "sending" ? "Joining…" : "Join the waitlist"}
        </button>
      </form>
      {total !== null && total > 0 && (
        <p className="font-mono text-xs text-ink-soft">
          {total} {total === 1 ? "person" : "people"} already waiting
        </p>
      )}
    </div>
  );
}

function AuthChip({
  open,
  onToggle,
}: {
  open: boolean;
  onToggle: () => void;
}) {
  const { data: session, isPending } = authClient.useSession();

  if (isPending) return null;

  if (session) {
    return (
      <div className="flex items-center gap-2 rounded-full border border-grid bg-white py-1.5 pr-3 pl-4 text-sm shadow-sm">
        <span>
          Hey, <span className="font-bold">{session.user.name}</span>{" "}
          <span aria-hidden>✏️</span>
        </span>
        <button
          onClick={() => authClient.signOut()}
          className="text-ink-soft underline-offset-2 hover:underline"
        >
          Sign out
        </button>
      </div>
    );
  }

  return (
    <button
      onClick={onToggle}
      className="rounded-full border border-grid bg-white px-4 py-1.5 text-sm font-medium shadow-sm transition hover:border-sky"
      aria-expanded={open}
    >
      {open ? "Close" : "Sign in"}
    </button>
  );
}

function AuthPanel() {
  const { data: session, isPending } = authClient.useSession();
  const [mode, setMode] = useState<"signin" | "signup">("signup");
  const [name, setName] = useState("");
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  if (isPending || session) return null;

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setError(null);
    const result =
      mode === "signup"
        ? await authClient.signUp.email({ name, email, password })
        : await authClient.signIn.email({ email, password });
    if (result.error) {
      setError(result.error.message ?? "Something went wrong — try again");
    }
    setBusy(false);
  };

  const field =
    "rounded-xl border-2 border-ink/10 bg-white px-4 py-2.5 text-ink placeholder-ink-soft/60 outline-none transition focus:border-sky";

  return (
    <form
      onSubmit={submit}
      className="flex w-full max-w-sm flex-col gap-3 rounded-2xl border border-grid bg-white p-6 shadow-[0_2px_12px_rgba(33,36,46,0.06)]"
    >
      <div className="flex gap-1 self-start rounded-full bg-canvas p-1">
        {(["signup", "signin"] as const).map((m) => (
          <button
            key={m}
            type="button"
            onClick={() => setMode(m)}
            className={`rounded-full px-3.5 py-1.5 text-sm font-medium transition ${
              mode === m
                ? "bg-ink text-white"
                : "text-ink-soft hover:text-ink"
            }`}
          >
            {m === "signup" ? "Create account" : "Sign in"}
          </button>
        ))}
      </div>
      {mode === "signup" && (
        <input
          required
          value={name}
          onChange={(e) => setName(e.target.value)}
          placeholder="Your name"
          className={field}
        />
      )}
      <input
        type="email"
        required
        value={email}
        onChange={(e) => setEmail(e.target.value)}
        placeholder="you@example.com"
        className={field}
      />
      <input
        type="password"
        required
        minLength={8}
        value={password}
        onChange={(e) => setPassword(e.target.value)}
        placeholder="Password (8+ characters)"
        className={field}
      />
      {error && <p className="text-sm text-alert">{error}</p>}
      <button
        type="submit"
        disabled={busy}
        className="rounded-xl bg-ink px-4 py-2.5 font-bold text-white transition hover:bg-ink/85 disabled:opacity-50"
      >
        {busy ? "One sec…" : mode === "signup" ? "Create account" : "Sign in"}
      </button>
    </form>
  );
}

export default function App() {
  const [authOpen, setAuthOpen] = useState(false);
  return (
    <div className="canvas-bg min-h-screen font-sans text-ink">
      <header className="fixed top-4 right-4 z-10">
        <AuthChip open={authOpen} onToggle={() => setAuthOpen((o) => !o)} />
      </header>
      <main className="mx-auto flex max-w-3xl flex-col items-center gap-12 px-6 py-24 text-center sm:py-32">
        <div className="relative">
          <Cursor
            name="you?"
            color="var(--color-copper)"
            className="cursor-drift-a -top-10 -right-16 hidden sm:block"
          />
          <Cursor
            name="us"
            color="var(--color-sky)"
            className="cursor-drift-b top-1/2 -left-32 hidden sm:block"
          />
          <SelectionBox>
            <h1 className="font-display text-6xl font-extrabold tracking-tight sm:text-7xl">
              etchable
            </h1>
          </SelectionBox>
        </div>

        <p className="max-w-xl text-lg leading-relaxed text-ink-soft">
          A friendly little tool for designing circuit boards. Sketch your
          idea, route your traces, <EtchWithTrace /> what matters.
        </p>

        <WaitlistForm />

        {authOpen && <AuthPanel />}

        <footer className="mt-10 font-mono text-xs text-ink-soft/80">
          <a
            href="/api/docs"
            className="underline-offset-4 hover:text-ink hover:underline"
          >
            API docs
          </a>
          <span className="mx-3" aria-hidden>
            ·
          </span>
          <a
            href="https://github.com/etchable/etchable"
            className="underline-offset-4 hover:text-ink hover:underline"
          >
            GitHub
          </a>
        </footer>
      </main>
    </div>
  );
}
