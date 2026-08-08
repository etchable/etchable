import { useEffect, useState, type CSSProperties, type FormEvent } from "react";
import { Button, Chip, Cursor, Input, Panel, SelectionBox } from "@etchable/ui";
import { api } from "./lib/api";
import { authClient } from "./lib/auth-client";

function EtchWithTrace() {
  return (
    <span className="relative inline-block">
      etch
      <svg
        className="marker-stroke absolute -bottom-1 left-0 w-full"
        viewBox="0 0 120 6"
        height="6"
        preserveAspectRatio="none"
        aria-hidden
      >
        <path
          d="M2 3 L 118 3"
          fill="none"
          stroke="var(--color-copper)"
          strokeWidth="3.5"
          strokeLinecap="round"
        />
      </svg>
    </span>
  );
}

// Background traces route in slowly, like a board filling itself in.
const TRACES: { d: string; via?: [number, number]; delay: number; dur: number }[] = [
  { d: "M -20 120 H 300 L 360 180 H 520", via: [524, 180], delay: 0.4, dur: 7 },
  { d: "M 1460 80 H 1150 L 1090 140 H 984", via: [980, 140], delay: 2.2, dur: 6 },
  { d: "M -20 700 H 200 L 260 640 H 376", via: [380, 640], delay: 4.5, dur: 7 },
  { d: "M 1460 760 H 1250 L 1190 700 H 1084", via: [1080, 700], delay: 6.5, dur: 6 },
  { d: "M 120 -20 V 200 L 180 260 V 376", via: [180, 380], delay: 8.5, dur: 6 },
  { d: "M 1320 -20 V 160 L 1260 220 V 316", via: [1260, 320], delay: 10.5, dur: 6 },
  { d: "M 400 920 V 780 L 460 720 V 656", via: [460, 652], delay: 12.5, dur: 6 },
  { d: "M 1040 920 V 800 L 980 740 V 680", via: [980, 676], delay: 14.5, dur: 6 },
  { d: "M -20 420 H 120 L 180 480 H 236", via: [240, 480], delay: 16.5, dur: 6 },
  { d: "M 1460 420 H 1330 L 1270 480 H 1210 L 1180 450", via: [1176, 446], delay: 18.5, dur: 7 },
];

function CircuitBackground() {
  return (
    <svg
      className="circuit pointer-events-none fixed inset-0 h-full w-full"
      viewBox="0 0 1440 900"
      preserveAspectRatio="xMidYMid slice"
      aria-hidden
    >
      {TRACES.map(({ d, via, delay, dur }) => (
        <g key={d} style={{ "--delay": `${delay}s`, "--dur": `${dur}s` } as CSSProperties}>
          <path d={d} />
          {via && <circle cx={via[0]} cy={via[1]} r="5" />}
        </g>
      ))}
    </svg>
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
        <Input
          id="waitlist-email"
          type="email"
          required
          value={email}
          onChange={(e) => setEmail(e.target.value)}
          placeholder="you@example.com"
          className="flex-1"
        />
        <Button type="submit" variant="copper" size="lg" disabled={status === "sending"}>
          {status === "sending" ? "Joining…" : "Join the waitlist"}
        </Button>
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
      <Chip className="py-1.5 pr-3 pl-4 text-sm">
        <span>
          Hey, <span className="font-bold">{session.user.name}</span>
        </span>
        <button
          onClick={() => authClient.signOut()}
          className="text-ink-soft underline-offset-2 hover:underline"
        >
          Sign out
        </button>
      </Chip>
    );
  }

  return (
    <Button onClick={onToggle} aria-expanded={open}>
      {open ? "Close" : "Sign in"}
    </Button>
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

  return (
    <Panel
      as="form"
      onSubmit={submit}
      className="flex w-full max-w-sm flex-col gap-3 p-6"
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
        <Input
          variant="field"
          inputSize="md"
          required
          value={name}
          onChange={(e) => setName(e.target.value)}
          placeholder="Your name"
        />
      )}
      <Input
        variant="field"
        inputSize="md"
        type="email"
        required
        value={email}
        onChange={(e) => setEmail(e.target.value)}
        placeholder="you@example.com"
      />
      <Input
        variant="field"
        inputSize="md"
        type="password"
        required
        minLength={8}
        value={password}
        onChange={(e) => setPassword(e.target.value)}
        placeholder="Password (8+ characters)"
      />
      {error && <p className="text-sm text-alert">{error}</p>}
      <Button type="submit" variant="ink" size="lg" disabled={busy}>
        {busy ? "One sec…" : mode === "signup" ? "Create account" : "Sign in"}
      </Button>
    </Panel>
  );
}

export default function App() {
  const [authOpen, setAuthOpen] = useState(false);
  return (
    <div className="canvas-bg min-h-screen font-sans text-ink">
      <CircuitBackground />
      <header className="fixed top-4 right-4 z-10">
        <AuthChip open={authOpen} onToggle={() => setAuthOpen((o) => !o)} />
      </header>
      <main className="relative z-10 mx-auto flex max-w-3xl flex-col items-center gap-12 px-6 py-24 text-center sm:py-32">
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
