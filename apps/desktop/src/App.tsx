import { useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { Button, IconCheck, IconX, Input, SelectionBox, Shell, Spinner } from "@etchable/ui";
import CircuitCanvas from "./circuit/CircuitCanvas";
import Chat from "./chat/Chat";
import { useEtchable } from "./state";
import "./App.css";

// The native title bar is hidden (titleBarStyle: Overlay), so the macOS
// traffic lights float over the shell titlebar's left edge; the Shell
// reserves room for them, but only when actually running in the Tauri
// webview on a Mac.
const macOverlayChrome =
  navigator.userAgent.includes("Mac") && "__TAURI_INTERNALS__" in window;

export default function App() {
  const app = useEtchable();
  const [pastedPath, setPastedPath] = useState("");
  const [creating, setCreating] = useState(false);
  const [projectName, setProjectName] = useState("");

  const pickProject = async () => {
    try {
      const picked = await open({ multiple: false, directory: true });
      if (typeof picked === "string") void app.openProject(picked);
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
        void app.createProject(parent, name);
      }
    } catch (err) {
      console.error("open dialog failed", err);
    }
  };

  const openPasted = (raw: string) => {
    const path = raw.trim();
    if (!path) return;
    if (path.endsWith(".zen")) void app.openBoard(path);
    else void app.openProject(path);
  };

  const { counts, building, build, source } = app;
  const hasBoard = build !== null || source !== null;

  const pill =
    "inline-flex items-center gap-1.5 whitespace-nowrap rounded-full px-2.5 py-[3px] font-mono text-[10.5px]";

  const titlebar = (
    <div className="flex h-full w-full items-center gap-2 pl-2 pr-1">
      <span className="mr-0.5 font-display text-sm font-extrabold tracking-tight">
        etchable
      </span>
      {app.project ? (
        <span
          className="max-w-[38vw] truncate rounded-full bg-ink/5 px-2.5 py-[3px] font-mono text-[10.5px] text-ink/55"
          title={app.project.root}
        >
          {app.project.name}
        </span>
      ) : source ? (
        <span
          className="max-w-[38vw] truncate rounded-full bg-ink/5 px-2.5 py-[3px] font-mono text-[10.5px] text-ink/55"
          title={source}
        >
          {source.split("/").slice(-2).join("/")}
        </span>
      ) : null}
      <span className="flex-1" />
      {building ? (
        <span className={`${pill} bg-ink/5 text-ink/55`}>
          <Spinner />
          building…
        </span>
      ) : build ? (
        counts.errors > 0 ? (
          <span className={`${pill} bg-alert/10 text-alert`}>
            <IconX size={11} /> {counts.errors} error{counts.errors === 1 ? "" : "s"}
          </span>
        ) : (
          <span className={`${pill} bg-leaf/10 text-leaf-deep`}>
            <IconCheck size={11} /> {counts.components} component
            {counts.components === 1 ? "" : "s"}
          </span>
        )
      ) : null}
      <Button variant="ghost" size="sm" disabled={!source || building} onClick={() => void app.rebuild()}>
        Rebuild
      </Button>
      <Button variant="copper" size="sm" onClick={() => void pickProject()}>
        Open project…
      </Button>
    </div>
  );

  const panel = (
    <div className="flex h-full min-h-0 flex-col">
      <Chat
        transcript={app.transcript}
        agentRunning={app.agentRunning}
        selection={app.selection}
        sessionInfo={app.sessionInfo}
        onSend={(t) => void app.sendMessage(t)}
        onRespondPermission={(id, allow) => void app.respondPermission(id, allow)}
        onInterrupt={app.interruptAgent}
        onNewSession={() => void app.newSession()}
        onClearSelection={() => app.setSelection([])}
      />
    </div>
  );

  return (
    <Shell
      macTrafficLights={macOverlayChrome}
      titlebar={titlebar}
      rightSidebar={hasBoard ? panel : undefined}
      rightMinWidth={340}
      defaultRightWidth={Math.max(360, Math.round(window.innerWidth * 0.3))}
    >
      {hasBoard ? (
        <>
          {app.boardError && (
            <div
              className="flex-none truncate border-b border-alert/25 bg-alert/5 px-3.5 py-1 font-mono text-[10.5px] text-alert"
              title={app.boardError}
            >
              {app.boardError}
            </div>
          )}
          <div className="flex min-h-0 flex-1">
            <CircuitCanvas
              view={app.display.view}
              source={source}
              dimmed={app.display.dimmed}
              diagnostics={app.diagnostics}
              selection={app.selection}
              onSelectionChange={app.setSelection}
              onSavePositions={app.savePositions}
            />
          </div>
        </>
      ) : (
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
            <Button variant="copper" size="lg" onClick={() => void pickProject()}>
              Open a project
            </Button>
            <Button variant="ghost" onClick={() => setCreating((v) => !v)}>
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
              <Button size="sm" disabled={!projectName.trim()} onClick={() => void createProjectAt()}>
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
            onChange={(e) => setPastedPath(e.currentTarget.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") openPasted(pastedPath);
            }}
          />
          {app.boardError && (
            <div className="mt-4 max-w-[420px] select-text whitespace-pre-wrap text-center font-mono text-xxs text-alert">
              {app.boardError}
            </div>
          )}
        </div>
      )}
    </Shell>
  );
}
