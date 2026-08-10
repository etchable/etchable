import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { open } from "@tauri-apps/plugin-dialog";
import { Button, IconX, Input, Shell, Spinner } from "@etchable/ui";
import type { RecentProject } from "./types";
import { macOverlayChrome } from "./chrome";
import "./App.css";

/** The copper trace under the hero: a run, a jog, a short run into a via.
    Draws in PIXEL coordinates at the measured width — the stroke never
    scales; narrow widths shorten the leading run instead. */
function TraceFlourish() {
  const holder = useRef<HTMLDivElement>(null);
  const [w, setW] = useState(0);
  useEffect(() => {
    const el = holder.current;
    if (!el) return;
    const measure = () => setW(Math.round(el.clientWidth));
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const yTop = 5;
  const yBot = 15;
  const viaR = 4.5;
  const cx = w - 2 - viaR;
  const tailEnd = cx - viaR - 1.5;
  const jogEnd = Math.max(18, w - 108);
  const jogStart = jogEnd - 14;

  return (
    <div ref={holder} className="mt-2 h-[22px] w-full max-w-[340px]" aria-hidden>
      {w > 80 && (
        <svg width={w} height={22} viewBox={`0 0 ${w} 22`} fill="none" className="text-copper">
          <path
            d={`M2 ${yTop} H${jogStart} L${jogEnd} ${yBot} H${tailEnd}`}
            stroke="currentColor"
            strokeWidth="3"
            strokeLinecap="round"
            strokeLinejoin="round"
          />
          <circle cx={cx} cy={yBot} r={viaR} stroke="currentColor" strokeWidth="3" />
        </svg>
      )}
    </div>
  );
}

function ago(ms: number): string {
  const s = (Date.now() - ms) / 1000;
  if (s < 60) return "just now";
  if (s < 3600) return `${Math.floor(s / 60)}m ago`;
  if (s < 86400) return `${Math.floor(s / 3600)}h ago`;
  return `${Math.floor(s / 86400)}d ago`;
}

/** The dashboard window: open, create, or paste a project, with the store's
    recent projects in the Shell's left sidebar. On success the backend
    shows the app window and hides this one, so everything here is local —
    just busy/error state around the open commands. */
const SKETCH_IDEAS = [
  "a USB-C power breakout, 5V at 3A",
  "an STM32 dev board with SWD",
  "a LiPo charger with fuel gauge",
];

export default function Dashboard() {
  const [pastedPath, setPastedPath] = useState("");
  const [creating, setCreating] = useState(false);
  const [projectName, setProjectName] = useState("");
  const nameRef = useRef<HTMLInputElement>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [recents, setRecents] = useState<RecentProject[]>([]);
  const [description, setDescription] = useState("");

  const refreshRecents = useCallback(() => {
    void invoke<RecentProject[]>("list_recent_projects")
      .then(setRecents)
      .catch((err) => console.warn("list_recent_projects failed", err));
  }, []);

  // The window is hidden, not destroyed, between uses — refresh the list
  // whenever it comes back.
  useEffect(() => {
    refreshRecents();
    window.addEventListener("focus", refreshRecents);
    return () => window.removeEventListener("focus", refreshRecents);
  }, [refreshRecents]);

  const run = async (cmd: string, args: Record<string, unknown>) => {
    setBusy(true);
    setError(null);
    try {
      await invoke(cmd, args);
      // The backend hid this window; reset so it comes back clean.
      setCreating(false);
      setProjectName("");
      setPastedPath("");
      refreshRecents();
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  };

  // Menu actions land here when the dashboard is the focused window. New
  // Project has no dialog of its own — the name is typed here — so the menu
  // opens the form and puts the cursor in it rather than inventing a prompt.
  useEffect(() => {
    const win = getCurrentWebviewWindow();
    const unlisten = win.listen<string>("menu-action", (e) => {
      if (e.payload === "open-project") void pickProject();
      if (e.payload === "new-project") {
        setCreating(true);
        requestAnimationFrame(() => nameRef.current?.focus());
      }
    });
    return () => {
      void unlisten.then((f) => f());
    };
  });

  const pickProject = async () => {
    try {
      // A project is identified by its etchable.toml — pick that file
      // (dialogs can only filter by extension, so the backend validates
      // the name). open_project also accepts a directory for pasted paths.
      const picked = await open({
        multiple: false,
        title: "Open a project (etchable.toml)",
        filters: [{ name: "etchable project", extensions: ["toml"] }],
      });
      if (typeof picked === "string") void run("open_project", { path: picked });
    } catch (err) {
      console.error("open dialog failed", err);
    }
  };

  const createProjectAt = async () => {
    const name = projectName.trim();
    if (!name) return;
    try {
      const parent = await open({
        multiple: false,
        directory: true,
        title: "Choose where to create the project",
      });
      if (typeof parent === "string") {
        setCreating(false);
        void run("create_project", { parent, name });
      }
    } catch (err) {
      console.error("open dialog failed", err);
    }
  };

  const openPasted = (raw: string) => {
    const path = raw.trim();
    if (!path) return;
    if (path.endsWith(".zen")) void run("select_board", { path });
    else void run("open_project", { path });
  };

  const removeRecent = (root: string) => {
    setRecents((rs) => rs.filter((r) => r.root !== root));
    void invoke("remove_recent_project", { root }).catch(() => refreshRecents());
  };

  const createRow = (
    <div className="mt-4 flex items-center gap-2">
      <Input
        ref={nameRef}
        inputSize="sm"
        type="text"
        placeholder="project name"
        autoFocus
        className="w-52"
        value={projectName}
        onChange={(e) => setProjectName(e.currentTarget.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") void createProjectAt();
        }}
      />
      <Button size="sm" disabled={!projectName.trim() || busy} onClick={() => void createProjectAt()}>
        Choose location…
      </Button>
    </div>
  );

  const pasteInput = (
    <Input
      mono
      inputSize="sm"
      type="text"
      placeholder="…or paste a project or board path"
      className="mt-4 w-80 text-center opacity-60 transition-opacity focus:opacity-100"
      value={pastedPath}
      disabled={busy}
      onChange={(e) => setPastedPath(e.currentTarget.value)}
      onKeyDown={(e) => {
        if (e.key === "Enter") openPasted(pastedPath);
      }}
    />
  );

  const statusRows = (
    <>
      {busy && (
        <div className="mt-4 flex items-center gap-1.5 font-mono text-[10.5px] text-ink/55">
          <Spinner />
          building…
        </div>
      )}
      {error && (
        <div className="mt-4 max-w-[420px] select-text whitespace-pre-wrap text-center font-mono text-xxs text-alert">
          {error}
        </div>
      )}
    </>
  );

  const titlebar = (
    <div className="flex h-full w-full items-center pl-2 pr-1">
      <span className="font-display text-sm font-extrabold tracking-tight">
        etchable
      </span>
    </div>
  );

  const sidebar = (
    <div className="flex h-full min-h-0 flex-col gap-1 px-2 py-2">
      <div className="px-1.5 pb-1 font-mono text-[10px] font-semibold tracking-wider text-ink/40 uppercase">
        Recent
      </div>
      {recents.length === 0 ? (
        <div className="mx-0.5 rounded-lg border border-dashed border-ink/15 px-3 py-3 text-xxs text-ink/40">
          Open a project and it'll live here.
        </div>
      ) : (
        <div className="scroll-minimal flex min-h-0 flex-1 flex-col gap-px overflow-y-auto">
          {recents.map((r) => (
            <div
              key={r.root}
              className="group flex min-w-0 items-center gap-1 rounded-lg px-1.5 py-1.5 transition-colors hover:bg-white"
            >
              <button
                type="button"
                disabled={busy}
                onClick={() => void run("open_project", { path: r.root })}
                className="min-w-0 flex-1 cursor-pointer text-left"
                title={r.root}
              >
                <div className="truncate text-xs font-semibold text-ink/85">{r.name}</div>
                <div className="truncate font-mono text-[10px] text-ink/40">
                  {r.root.split("/").slice(-2).join("/")}
                </div>
              </button>
              <button
                type="button"
                className="flex flex-none cursor-pointer rounded p-1 text-ink/0 transition-colors group-hover:text-ink/40 hover:!text-ink/80"
                title="Remove from recents"
                onClick={() => removeRecent(r.root)}
              >
                <IconX size={11} />
              </button>
            </div>
          ))}
        </div>
      )}
    </div>
  );

  return (
    <Shell
      macTrafficLights={macOverlayChrome}
      titlebar={titlebar}
      leftSidebar={sidebar}
      leftMinWidth={190}
      defaultLeftWidth={230}
    >
      {recents.length === 0 ? (
        /* First board: the full welcome hero. Scrolls (m-auto inside an
           overflow container) instead of clipping when the window is short —
           justify-center would push the top out of reach. */
        <div className="dotgrid flex min-h-0 flex-1 flex-col overflow-y-auto">
          <div className="m-auto flex w-full flex-col items-center px-8 py-10">
          <h1 className="text-center font-display text-4xl font-extrabold tracking-tight">
            Etch your first board
          </h1>
          <TraceFlourish />
          <p className="mt-5 max-w-[440px] text-center text-[13px] leading-relaxed text-ink/55">
            Sketch your idea, route your traces, etch what matters.
            <br />
            Your board lives in a git repo from the first wire.
          </p>
          <div className="mt-8 flex flex-wrap items-center justify-center gap-5">
            <Button
              variant="copper"
              size="lg"
              style={{ borderRadius: 14 }}
              disabled={busy}
              onClick={() => setCreating((v) => !v)}
            >
              Create a board
            </Button>
            <button
              type="button"
              disabled={busy}
              className="cursor-pointer text-sm font-bold text-ink transition-colors hover:text-copper disabled:opacity-50"
              onClick={() => void pickProject()}
            >
              Open a project
            </button>
          </div>
          {creating && createRow}

          <div className="mt-12 font-mono text-[11px] font-medium tracking-[0.25em] text-ink/45 uppercase">
            or ask the agent
          </div>
          <div className="mt-4 flex w-full max-w-[600px] items-center gap-2 rounded-full border border-ink/10 bg-white p-1.5 pl-5 shadow-island transition-shadow focus-within:shadow-island-lg focus-within:ring-2 focus-within:ring-copper">
            <input
              type="text"
              className="dash-agent-input min-w-0 flex-1 bg-transparent text-[13.5px] outline-none placeholder:text-ink/35"
              placeholder="Describe a board — the agent sketches the schematic for you to review"
              value={description}
              disabled={busy}
              onChange={(e) => setDescription(e.currentTarget.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter" && description.trim()) {
                  void run("sketch_board", { description: description.trim() });
                }
              }}
            />
            <Button
              variant="copper"
              disabled={busy || !description.trim()}
              onClick={() => void run("sketch_board", { description: description.trim() })}
            >
              Sketch it
            </Button>
          </div>
          <div className="mt-4 flex flex-wrap items-center justify-center gap-2.5">
            {SKETCH_IDEAS.map((idea) => (
              <button
                key={idea}
                type="button"
                disabled={busy}
                className="cursor-pointer rounded-full border border-ink/12 bg-white px-4 py-1.5 text-xs font-medium text-ink/80 transition-colors hover:border-copper/50 hover:text-ink disabled:opacity-50"
                onClick={() => setDescription(idea)}
              >
                {idea}
              </button>
            ))}
          </div>
          {statusRows}
          </div>
        </div>
      ) : (
        /* Returning: the boards themselves are the main content. */
        <div className="dotgrid flex flex-1 flex-col overflow-y-auto px-8 py-7">
          <div className="mx-auto w-full max-w-[660px]">
            <div className="flex items-center gap-2">
              <h1 className="font-display text-2xl font-extrabold tracking-tight">
                Etch a board
              </h1>
              <span className="flex-1" />
              <Button variant="ghost" size="sm" disabled={busy} onClick={() => void pickProject()}>
                Open a project
              </Button>
              <Button
                variant="copper"
                size="sm"
                disabled={busy}
                onClick={() => setCreating((v) => !v)}
              >
                Create a board
              </Button>
            </div>
            {creating && createRow}
            <div className="mt-5 grid grid-cols-2 gap-3">
              {recents.map((r) => (
                <div
                  key={r.root}
                  className="group relative rounded-xl bg-white p-3 shadow-island transition-shadow hover:shadow-island-lg"
                >
                  <button
                    type="button"
                    disabled={busy}
                    onClick={() => void run("open_project", { path: r.root })}
                    className="block w-full min-w-0 cursor-pointer text-left"
                    title={r.root}
                  >
                    <div className="truncate text-sm font-semibold text-ink/90">{r.name}</div>
                    <div className="mt-0.5 truncate font-mono text-[10px] text-ink/40">
                      {r.root.split("/").slice(-2).join("/")}
                    </div>
                    <div className="mt-2 font-mono text-[10px] text-ink/35">
                      {ago(r.lastOpenedAt)}
                    </div>
                  </button>
                  <button
                    type="button"
                    className="absolute right-2 top-2 flex cursor-pointer rounded p-1 text-ink/0 transition-colors group-hover:text-ink/35 hover:!text-ink/80"
                    title="Remove from recents"
                    onClick={() => removeRecent(r.root)}
                  >
                    <IconX size={11} />
                  </button>
                </div>
              ))}
            </div>
            <div className="mt-5 flex flex-col items-center">
              {pasteInput}
              {statusRows}
            </div>
          </div>
        </div>
      )}
    </Shell>
  );
}
