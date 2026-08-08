import { useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import CircuitCanvas from "./circuit/CircuitCanvas";
import Chat from "./chat/Chat";
import Problems from "./problems/Problems";
import { useEtchable } from "./state";
import type { Diag } from "./types";
import "./App.css";

function clamp(v: number, lo: number, hi: number): number {
  return Math.min(hi, Math.max(lo, v));
}

function fileMatches(a: string | null | undefined, b: string | undefined): boolean {
  if (!a || !b) return false;
  return a === b || a.endsWith("/" + b) || b.endsWith("/" + a);
}

export default function App() {
  const app = useEtchable();
  const [tab, setTab] = useState<"chat" | "problems">("chat");
  const [panelW, setPanelW] = useState(() =>
    Math.max(360, Math.round(window.innerWidth * 0.3)),
  );
  const [pastedPath, setPastedPath] = useState("");
  const dividerDragging = useRef(false);

  const pickBoard = async () => {
    try {
      const picked = await open({
        multiple: false,
        filters: [{ name: "Zener", extensions: ["zen"] }],
      });
      if (typeof picked === "string") void app.openBoard(picked);
    } catch (err) {
      console.error("open dialog failed", err);
    }
  };

  const handleSelectDiag = (diag: Diag) => {
    const sch = app.display.view?.schematic;
    if (!sch || !diag.file) return;
    const hits: string[] = [];
    for (const inst of Object.values(sch.instances)) {
      if (inst.kind === "component" && fileMatches(inst.source_file, diag.file)) {
        hits.push(inst.path);
      }
    }
    if (hits.length > 0) app.setSelection(hits);
  };

  const onDividerDown = (e: React.PointerEvent<HTMLDivElement>) => {
    dividerDragging.current = true;
    e.currentTarget.setPointerCapture(e.pointerId);
  };
  const onDividerMove = (e: React.PointerEvent<HTMLDivElement>) => {
    if (!dividerDragging.current) return;
    setPanelW(clamp(window.innerWidth - e.clientX, 340, window.innerWidth * 0.7));
  };
  const onDividerUp = () => {
    dividerDragging.current = false;
  };

  const { counts, building, build, source } = app;
  const problemCount = counts.errors + counts.warnings;
  const hasBoard = build !== null || source !== null;

  return (
    <div className="app">
      <header className="toolbar">
        <span className="brand">etchable</span>
        {source && (
          <span className="board-path" title={source}>
            {source}
          </span>
        )}
        <span className="toolbar-spacer" />
        {building ? (
          <span className="pill pill-building">
            <span className="spinner" />
            building…
          </span>
        ) : build ? (
          counts.errors > 0 ? (
            <span className="pill pill-err">✗ {counts.errors} error{counts.errors === 1 ? "" : "s"}</span>
          ) : (
            <span className="pill pill-ok">
              ✓ {counts.components} component{counts.components === 1 ? "" : "s"}
            </span>
          )
        ) : null}
        <button
          type="button"
          className="btn"
          disabled={!source || building}
          onClick={() => void app.rebuild()}
        >
          Rebuild
        </button>
        <button type="button" className="btn btn-primary" onClick={() => void pickBoard()}>
          Open board…
        </button>
      </header>

      {app.boardError && hasBoard && (
        <div className="error-strip" title={app.boardError}>
          {app.boardError}
        </div>
      )}

      {hasBoard ? (
        <div className="main">
          <CircuitCanvas
            view={app.display.view}
            source={source}
            dimmed={app.display.dimmed}
            diagnostics={app.diagnostics}
            selection={app.selection}
            onSelectionChange={app.setSelection}
          />
          <div
            className="divider"
            onPointerDown={onDividerDown}
            onPointerMove={onDividerMove}
            onPointerUp={onDividerUp}
          />
          <aside className="panel" style={{ width: panelW }}>
            <div className="tabs">
              <button
                type="button"
                className={"tab" + (tab === "chat" ? " active" : "")}
                onClick={() => setTab("chat")}
              >
                Chat
              </button>
              <button
                type="button"
                className={"tab" + (tab === "problems" ? " active" : "")}
                onClick={() => setTab("problems")}
              >
                Problems
                {problemCount > 0 && (
                  <span className={"tab-badge" + (counts.errors > 0 ? " has-err" : "")}>
                    {problemCount}
                  </span>
                )}
              </button>
            </div>
            <div className="panel-body">
              {tab === "chat" ? (
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
              ) : (
                <Problems diagnostics={app.diagnostics} onSelectDiag={handleSelectDiag} />
              )}
            </div>
          </aside>
        </div>
      ) : (
        <div className="empty-state">
          <div className="empty-card">
            <div className="empty-title">etchable</div>
            <div className="empty-sub">Infinite-canvas schematic viewer for Zener boards</div>
            <button type="button" className="btn btn-primary btn-lg" onClick={() => void pickBoard()}>
              Open a .zen board
            </button>
            <div className="empty-hint">
              A demo board ships with the repo at{" "}
              <code>examples/demo/top.zen</code> — or paste a path below.
            </div>
            <div className="empty-path-row">
              <input
                className="path-input"
                type="text"
                placeholder="/path/to/board.zen"
                value={pastedPath}
                onChange={(e) => setPastedPath(e.currentTarget.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter" && pastedPath.trim()) {
                    void app.openBoard(pastedPath.trim());
                  }
                }}
              />
              <button
                type="button"
                className="btn"
                disabled={pastedPath.trim().length === 0}
                onClick={() => void app.openBoard(pastedPath.trim())}
              >
                Open
              </button>
            </div>
            {app.boardError && <div className="empty-error">{app.boardError}</div>}
          </div>
        </div>
      )}
    </div>
  );
}
