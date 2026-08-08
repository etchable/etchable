import { useEffect, useState, type FormEvent } from "react";
import { api } from "./lib/api";
import { authClient } from "./lib/auth-client";

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
      <p className="rounded-2xl bg-emerald-50 px-6 py-4 text-emerald-700">
        {status === "done"
          ? "You're on the list! We'll be in touch soon. 💌"
          : "You're already on the list — we haven't forgotten you! 💌"}
      </p>
    );
  }

  return (
    <form onSubmit={submit} className="flex w-full max-w-md gap-2">
      <input
        type="email"
        required
        value={email}
        onChange={(e) => setEmail(e.target.value)}
        placeholder="you@example.com"
        className="min-w-0 flex-1 rounded-xl border border-stone-300 bg-white px-4 py-3 text-stone-800 placeholder-stone-400 outline-none focus:border-amber-400 focus:ring-2 focus:ring-amber-200"
      />
      <button
        type="submit"
        disabled={status === "sending"}
        className="rounded-xl bg-amber-500 px-5 py-3 font-medium text-white transition hover:bg-amber-600 disabled:opacity-50"
      >
        {status === "sending" ? "Joining…" : "Join the waitlist"}
      </button>
      {total !== null && (
        <span className="sr-only">{total} people are already waiting</span>
      )}
    </form>
  );
}

function AuthCard() {
  const { data: session, isPending } = authClient.useSession();
  const [mode, setMode] = useState<"signin" | "signup">("signup");
  const [name, setName] = useState("");
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  if (isPending) return null;

  if (session) {
    return (
      <div className="flex items-center gap-3 rounded-2xl border border-stone-200 bg-white/70 px-5 py-3 shadow-sm backdrop-blur">
        <span className="text-stone-700">
          Hey, <span className="font-semibold">{session.user.name}</span> 👋
        </span>
        <button
          onClick={() => authClient.signOut()}
          className="text-sm text-stone-500 underline-offset-2 hover:underline"
        >
          Sign out
        </button>
      </div>
    );
  }

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setError(null);
    const result =
      mode === "signup"
        ? await authClient.signUp.email({ name, email, password })
        : await authClient.signIn.email({ email, password });
    if (result.error) {
      setError(result.error.message ?? "Something went wrong");
    }
    setBusy(false);
  };

  return (
    <form
      onSubmit={submit}
      className="flex w-full max-w-sm flex-col gap-3 rounded-2xl border border-stone-200 bg-white/70 p-6 shadow-sm backdrop-blur"
    >
      <div className="flex gap-4 text-sm">
        <button
          type="button"
          onClick={() => setMode("signup")}
          className={
            mode === "signup"
              ? "font-semibold text-amber-600"
              : "text-stone-500 hover:text-stone-700"
          }
        >
          Create account
        </button>
        <button
          type="button"
          onClick={() => setMode("signin")}
          className={
            mode === "signin"
              ? "font-semibold text-amber-600"
              : "text-stone-500 hover:text-stone-700"
          }
        >
          Sign in
        </button>
      </div>
      {mode === "signup" && (
        <input
          required
          value={name}
          onChange={(e) => setName(e.target.value)}
          placeholder="Your name"
          className="rounded-xl border border-stone-300 bg-white px-4 py-2.5 outline-none focus:border-amber-400 focus:ring-2 focus:ring-amber-200"
        />
      )}
      <input
        type="email"
        required
        value={email}
        onChange={(e) => setEmail(e.target.value)}
        placeholder="you@example.com"
        className="rounded-xl border border-stone-300 bg-white px-4 py-2.5 outline-none focus:border-amber-400 focus:ring-2 focus:ring-amber-200"
      />
      <input
        type="password"
        required
        minLength={8}
        value={password}
        onChange={(e) => setPassword(e.target.value)}
        placeholder="Password (8+ characters)"
        className="rounded-xl border border-stone-300 bg-white px-4 py-2.5 outline-none focus:border-amber-400 focus:ring-2 focus:ring-amber-200"
      />
      {error && <p className="text-sm text-red-600">{error}</p>}
      <button
        type="submit"
        disabled={busy}
        className="rounded-xl bg-stone-800 px-4 py-2.5 font-medium text-white transition hover:bg-stone-900 disabled:opacity-50"
      >
        {busy ? "One sec…" : mode === "signup" ? "Create account" : "Sign in"}
      </button>
    </form>
  );
}

export default function App() {
  return (
    <div className="min-h-screen bg-gradient-to-b from-amber-50 via-stone-50 to-white text-stone-900">
      <main className="mx-auto flex max-w-3xl flex-col items-center gap-10 px-6 py-24 text-center">
        <span className="rounded-full border border-amber-200 bg-amber-100 px-4 py-1 text-sm text-amber-700">
          ✨ Coming soon
        </span>
        <h1 className="text-5xl font-bold tracking-tight sm:text-6xl">
          etchable
        </h1>
        <p className="max-w-xl text-lg text-stone-600">
          Make your mark. etchable is a friendly little tool for turning your
          ideas into something permanent — coming soon to your desktop.
        </p>
        <WaitlistForm />
        <AuthCard />
        <footer className="mt-16 text-sm text-stone-400">
          <a
            href="/api/docs"
            className="underline-offset-2 hover:text-stone-600 hover:underline"
          >
            API docs
          </a>
          <span className="mx-2">·</span>
          <a
            href="https://github.com/etchable/etchable"
            className="underline-offset-2 hover:text-stone-600 hover:underline"
          >
            GitHub
          </a>
        </footer>
      </main>
    </div>
  );
}
