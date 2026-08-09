import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { Button, IconX, Input, SelectionBox, Shell, Spinner } from "@etchable/ui";
import type { RecentProject } from "./types";
import { macOverlayChrome } from "./chrome";
import "./App.css";

/** The dashboard window: open, create, or paste a project. On success the
    backend shows the app window and hides this one, so everything here is
    local — just busy/error state around the open commands. */
export default function Dashboard() {
  const [pastedPath, setPastedPath] = useState("");
  const [creating, setCreating] = useState(false);
  const [projectName, setProjectName] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [recents, setRecents] = useState<RecentProject[]>([]);

  // Recents come from ~/.etchable/state; refresh on mount and whenever the
  // dashboard comes back into focus (it hides while project windows are up,
  // and opens recorded meanwhile should show when it returns).
  const refreshRecents = useCallback(() => {
    void invoke<RecentProject[]>("list_recent_projects")
      .then(setRecents)
      .catch((err) => console.warn("list_recent_projects failed", err));
  }, []);
  useEffect(() => {
    refreshRecents();
    window.addEventListener("focus", refreshRecents);
    return () => window.removeEventListener("focus", refreshRecents);
  }, [refreshRecents]);

  const removeRecent = (root: string) => {
    setRecents((rs) => rs.filter((r) => r.root !== root));
    void invoke("remove_recent_project", { root }).catch(() => refreshRecents());
  };

  const run = async (cmd: string, args: Record<string, unknown>) => {
    setBusy(true);
    setError(null);
    try {
      await invoke(cmd, args);
      // The backend hid this window; reset so it comes back clean.
      setCreating(false);
      setProjectName("");
      setPastedPath("");
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  };

  const pickProject = async () => {
    try {
      const picked = await open({ multiple: false, directory: true });
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

  const titlebar = (
    <div className="flex h-full w-full items-center pl-2 pr-1">
      <span className="font-display text-sm font-extrabold tracking-tight">
        etchable
      </span>
    </div>
  );

  return (
    <Shell macTrafficLights={macOverlayChrome} titlebar={titlebar}>
      <div className="dotgrid flex flex-1 flex-col items-center justify-center">
        <SelectionBox>
          <h1 className="font-display text-6xl font-extrabold tracking-tight">
            etchable
          </h1>
        </SelectionBox>
        <p className="mt-2 text-[13px] text-ink/55">
          A friendly little tool for designing circuit boards.
        </p>
        <div className="mt-8 flex items-center gap-3">
          <Button variant="copper" size="lg" disabled={busy} onClick={() => void pickProject()}>
            Open a project
          </Button>
          <Button variant="ghost" disabled={busy} onClick={() => setCreating((v) => !v)}>
            New project
          </Button>
        </div>
        {creating && (
          <div className="mt-4 flex items-center gap-2">
            <Input
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
        )}
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
        {recents.length > 0 && (
          <div className="mt-8 w-[420px]">
            <div className="mb-1.5 px-1 text-xxs font-medium uppercase tracking-wide text-ink/35">
              Recent projects
            </div>
            <div className="flex flex-col gap-0.5">
              {recents.slice(0, 6).map((r) => (
                <div
                  key={r.root}
                  className="group flex cursor-pointer items-center gap-2 rounded-lg px-2.5 py-1.5 transition-colors hover:bg-ink/4"
                  onClick={() => {
                    if (!busy) void run("open_project", { path: r.root });
                  }}
                >
                  <span className="text-xs font-medium">{r.name}</span>
                  <span className="min-w-0 flex-1 truncate font-mono text-[10px] text-ink/35">
                    {r.root}
                  </span>
                  <button
                    type="button"
                    aria-label={`Remove ${r.name} from recents`}
                    className="flex flex-none cursor-pointer text-ink/25 opacity-0 transition-opacity hover:text-ink/55 group-hover:opacity-100"
                    onClick={(e) => {
                      e.stopPropagation();
                      removeRecent(r.root);
                    }}
                  >
                    <IconX size={12} />
                  </button>
                </div>
              ))}
            </div>
          </div>
        )}
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
      </div>
    </Shell>
  );
}
