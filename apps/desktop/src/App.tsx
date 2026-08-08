import { useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { Button, IconCheck, IconX, Input, Panel, Spinner } from "@etchable/ui";
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

  const pill =
    "inline-flex items-center gap-1.5 whitespace-nowrap rounded-full px-2.5 py-[3px] font-mono text-[10.5px]";

  return (
    <div className="flex h-full flex-col">
      <header className="flex h-11 flex-none items-center gap-2 border-b border-ink/5 bg-chrome pl-3.5 pr-2.5">
        <span className="mr-0.5 font-display text-sm font-extrabold tracking-tight">
          etchable
        </span>
        {source && (
          <span
            className="max-w-[38vw] truncate rounded-full bg-ink/5 px-2.5 py-[3px] font-mono text-[10.5px] text-ink/55"
            title={source}
          >
            {source.split("/").slice(-2).join("/")}
          </span>
        )}
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
        <Button variant="copper" size="sm" onClick={() => void pickBoard()}>
          Open board…
        </Button>
      </header>

      {app.boardError && hasBoard && (
        <div
          className="flex-none truncate border-b border-alert/25 bg-alert/5 px-3.5 py-1 font-mono text-[10.5px] text-alert"
          title={app.boardError}
        >
          {app.boardError}
        </div>
      )}

      {hasBoard ? (
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
          <div
            className="w-[5px] flex-none cursor-col-resize transition-colors hover:bg-sky/10"
            onPointerDown={onDividerDown}
            onPointerMove={onDividerMove}
            onPointerUp={onDividerUp}
          />
          <aside
            className="flex min-h-0 min-w-[340px] flex-none flex-col border-l border-ink/8 bg-white"
            style={{ width: panelW }}
          >
            <div className="flex-none px-2.5 pb-1.5 pt-2">
              <div className="inline-flex gap-0.5 rounded-lg bg-ink/5 p-0.5">
                {(["chat", "problems"] as const).map((t) => (
                  <button
                    key={t}
                    type="button"
                    className={
                      "inline-flex cursor-pointer items-center gap-1.5 rounded-md px-3 py-[3px] text-xxs font-medium transition-colors " +
                      (tab === t ? "bg-white text-ink/92 shadow-seg" : "text-ink/55 hover:text-ink/92")
                    }
                    onClick={() => setTab(t)}
                  >
                    {t === "chat" ? "Chat" : "Problems"}
                    {t === "problems" && problemCount > 0 && (
                      <span
                        className={
                          "rounded-full px-[5px] font-mono text-[9.5px] " +
                          (counts.errors > 0 ? "bg-alert/10 text-alert" : "bg-warn/15 text-warn-deep")
                        }
                      >
                        {problemCount}
                      </span>
                    )}
                  </button>
                ))}
              </div>
            </div>
            <div className="flex min-h-0 flex-1 flex-col">
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
        <div className="dotgrid flex flex-1 items-center justify-center">
          <Panel className="flex max-w-[460px] flex-col items-center gap-3.5 px-11 py-9">
            <div className="font-display text-[26px] font-extrabold tracking-tight">
              etchable
            </div>
            <div className="text-[13px] text-ink/55">
              Infinite-canvas schematic viewer for Zener boards
            </div>
            <Button variant="copper" size="lg" onClick={() => void pickBoard()}>
              Open a .zen board
            </Button>
            <div className="text-center text-xs text-ink/35">
              A demo board ships with the repo at{" "}
              <code className="rounded bg-elev px-[5px] py-px font-mono text-ink/55">
                examples/demo/top.zen
              </code>{" "}
              — or paste a path below.
            </div>
            <div className="flex w-full gap-2">
              <Input
                mono
                inputSize="sm"
                type="text"
                placeholder="/path/to/board.zen"
                className="flex-1"
                value={pastedPath}
                onChange={(e) => setPastedPath(e.currentTarget.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter" && pastedPath.trim()) {
                    void app.openBoard(pastedPath.trim());
                  }
                }}
              />
              <Button
                size="sm"
                disabled={pastedPath.trim().length === 0}
                onClick={() => void app.openBoard(pastedPath.trim())}
              >
                Open
              </Button>
            </div>
            {app.boardError && (
              <div className="max-w-[380px] select-text whitespace-pre-wrap font-mono text-xxs text-alert">
                {app.boardError}
              </div>
            )}
          </Panel>
        </div>
      )}
    </div>
  );
}
