import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import {
  Button,
  IconCheck,
  IconSquaresFour,
  IconX,
  Shell,
  Spinner,
} from "@etchable/ui";
import CircuitCanvas from "./circuit/CircuitCanvas";
import Chat from "./chat/Chat";
import { useEtchable } from "./state";
import { macOverlayChrome } from "./chrome";
import "./App.css";

/** The app window: schematic canvas + agent chat for the open board. The
    dashboard (open/create a project) lives in its own window; this one is
    hidden until a board is open, so the no-board state below is only a
    fallback. */
export default function App() {
  const app = useEtchable();

  const pickProject = async () => {
    try {
      const picked = await open({ multiple: false, directory: true });
      if (typeof picked === "string") void app.openProject(picked);
    } catch (err) {
      console.error("open dialog failed", err);
    }
  };

  const showDashboard = () => {
    void invoke("show_dashboard").catch((err) =>
      console.error("show_dashboard failed", err),
    );
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
      {/* Same register as the Shell's sidebar collapsers: a bare icon
          button, not a labeled action. */}
      <button
        type="button"
        className="shell-collapser"
        aria-label="Open the dashboard"
        title="Dashboard"
        onClick={showDashboard}
      >
        <IconSquaresFour size={16} />
      </button>
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
        sessions={app.sessions}
        onSend={(t) => void app.sendMessage(t)}
        onRespondPermission={(id, allow) => void app.respondPermission(id, allow)}
        onInterrupt={app.interruptAgent}
        onNewSession={() => void app.newSession()}
        onClearSelection={() => app.setSelection([])}
        onResumeSession={(id) => void app.resumeSession(id)}
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
        <div className="dotgrid flex flex-1 flex-col items-center justify-center gap-4">
          <p className="text-[13px] text-ink/55">No board open.</p>
          <Button variant="copper" size="sm" onClick={showDashboard}>
            Open the dashboard
          </Button>
        </div>
      )}
    </Shell>
  );
}
