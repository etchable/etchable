import { useCallback, useEffect, useRef, useState } from "react";
import ErrorBoundary from "./ErrorBoundary";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import {
  Button,
  IconSquaresFour,
  IconX,
  Shell,
  Spinner,
  type ShellApi,
} from "@etchable/ui";
import CircuitCanvas from "./circuit/CircuitCanvas";
import Palette from "./circuit/Palette";
import type { Provisional } from "./circuit/ProvisionalLayer";
import Chat from "./chat/Chat";
import { useEtchable } from "./state";
import { macOverlayChrome } from "./chrome";
import type { LabelArm, PlacementArm } from "./types";
import "./App.css";

/** The app window: schematic canvas + agent chat for the open board. The
    dashboard (open/create a project) lives in its own window; this one is
    hidden until a board is open, so the no-board state below is only a
    fallback. */
export default function App() {
  const app = useEtchable();

  // Canvas tools (decision 0009): placement (phase 1) and net labeling
  // (phase 2). Arming one disarms the other; re-clicking an armed item
  // toggles it off.
  const [placement, setPlacement] = useState<PlacementArm | null>(null);
  const [labelMode, setLabelMode] = useState<LabelArm | null>(null);
  const armPlacement = useCallback(
    (arm: PlacementArm) => {
      setLabelMode(null);
      setPlacement((prev) => (prev?.spec === arm.spec ? null : arm));
      // Pre-warm the preflight while the user aims: the drop then commits
      // without evaluator round-trips, and the ghost gets the real shape.
      void app.warmPlacement(arm.spec).then((ghost) => {
        if (!ghost) return;
        setPlacement((prev) => (prev?.spec === arm.spec ? { ...prev, ghost } : prev));
      });
    },
    [app],
  );
  const armLabel = useCallback((arm: LabelArm) => {
    setPlacement(null);
    setLabelMode((prev) => (prev?.label === arm.label ? null : arm));
  }, []);

  const {
    addInstance,
    attachPinNet,
    renameNet,
    connectPins,
    setAttribute,
    renameInstance,
    removeInstances,
    disconnectPin,
    display: appDisplay,
  } = app;
  // The LATEST build's hash — even when the display shows the last good
  // build (red board), gestures edit the CURRENT file; the last-good hash
  // would bounce every one of them as stale.
  const baseHash = app.build?.source_hash ?? appDisplay.view?.source_hash;

  // Provisional parts: placed and written, not yet in a clean build. They
  // render as movable dashed stand-ins so a red build never hides work.
  const [provisionals, setProvisionals] = useState<Provisional[]>([]);
  useEffect(() => {
    // A schematic-bearing build settles the provisionals it CONTAINS (the
    // real symbol takes over). Ones it doesn't contain stay — with rapid
    // chained drops, a part written mid-build isn't in this build yet and
    // must not blink out. Undo clears its own optimism below.
    const sch = app.build?.schematic;
    if (!sch) return;
    setProvisionals((prev) => prev.filter((p) => !sch.instances[`root.${p.name}`]));
  }, [app.build]);
  const commitPlacement = useCallback(
    async (
      name: string,
      attrs: [string, string][],
      position: { x: number; y: number; rotation: number },
    ) => {
      if (!placement) return;
      if (!baseHash) throw new Error("no build to place against");
      const res = await addInstance(placement.spec, name, attrs, position, baseHash);
      setProvisionals((prev) => [
        ...prev.filter((p) => p.name !== name),
        {
          name,
          label: placement.label,
          pins: res.pins,
          x: position.x,
          y: position.y,
          rotation: position.rotation,
          positionPath: res.position_key ? `root.${res.position_key}` : null,
          ghost: res.ghost,
        },
      ]);
    },
    [placement, addInstance, baseHash],
  );
  const commitAttach = useCallback(
    async (path: string, pin: string, netName: string, kind: string) => {
      if (!baseHash) throw new Error("no build to edit against");
      await attachPinNet(path, pin, netName, kind, baseHash);
    },
    [attachPinNet, baseHash],
  );
  const commitRenameNet = useCallback(
    async (from: string, to: string) => {
      if (!baseHash) throw new Error("no build to edit against");
      await renameNet(from, to, baseHash);
    },
    [renameNet, baseHash],
  );
  const commitSetAttribute = useCallback(
    async (path: string, key: string, value: string) => {
      if (!baseHash) throw new Error("no build to edit against");
      await setAttribute(path, key, value, baseHash);
    },
    [setAttribute, baseHash],
  );
  const commitRenameInstance = useCallback(
    async (from: string, to: string) => {
      if (!baseHash) throw new Error("no build to edit against");
      await renameInstance(from, to, baseHash);
    },
    [renameInstance, baseHash],
  );
  const commitRemove = useCallback(
    async (paths: string[]) => {
      if (!baseHash) throw new Error("no build to edit against");
      await removeInstances(paths, baseHash);
    },
    [removeInstances, baseHash],
  );
  const commitConnect = useCallback(
    async (
      a: { path: string; pin: string },
      b: { path: string; pin: string },
      allowMerge: boolean,
    ) => {
      if (!baseHash) throw new Error("no build to edit against");
      return await connectPins(a, b, allowMerge, baseHash);
    },
    [connectPins, baseHash],
  );

  const pickProject = async () => {
    try {
      // Projects are identified by etchable.toml; pick the manifest file
      // (extension-only filtering — the backend validates the name).
      const picked = await open({
        multiple: false,
        title: "Open a project (etchable.toml)",
        filters: [{ name: "etchable project", extensions: ["toml"] }],
      });
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
  const shellApiRef = useRef<ShellApi | null>(null);
  // A prompt waiting in the composer, never auto-sent. `seq` lets the same
  // text be re-offered after the user clears it.
  const [draft, setDraft] = useState<{ text: string; seq: number } | null>(null);
  const draftSeq = useRef(0);
  /** Hand the agent the failing build, with the panel open to see it. */
  const askAgentToFix = useCallback(() => {
    const errors = app.diagnostics.filter((d) => d.severity === "error");
    if (errors.length === 0) return;
    const lines = errors.slice(0, 10).map((d) => {
      const where = d.file ? `${d.file}${d.line ? `:${d.line}` : ""}` : "";
      return where ? `- ${where} — ${d.message}` : `- ${d.message}`;
    });
    const more = errors.length > lines.length ? `\n- …and ${errors.length - lines.length} more` : "";
    draftSeq.current += 1;
    setDraft({
      text: `Fix ${errors.length === 1 ? "this build error" : "these build errors"}:\n\n${lines.join("\n")}${more}`,
      seq: draftSeq.current,
    });
    shellApiRef.current?.openRight();
  }, [app.diagnostics]);
  const hasBoard = build !== null || source !== null;

  const pill =
    "inline-flex items-center gap-1.5 whitespace-nowrap rounded-full px-2.5 py-[3px] font-mono text-[10.5px]";

  const titlebar = (
    <div className="flex h-full w-full items-center gap-2 pl-2 pr-1">
      <span className="mr-0.5 font-display text-sm font-extrabold tracking-tight">
        etchable
      </span>
      {/* A labeled action, deliberately NOT in the sidebar-collapser
          register: a bare icon here read as chrome toggling, but this
          navigates to the dashboard window. */}
      <Button variant="ghost" size="sm" onClick={showDashboard}>
        <IconSquaresFour size={13} />
        Dashboard
      </Button>
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
      ) : build && counts.errors > 0 ? (
        <button
          type="button"
          className={`${pill} cursor-pointer bg-alert/10 text-alert transition-colors hover:bg-alert/20`}
          title="Ask Claude to fix — fills the chat with the errors, ready to send"
          onClick={askAgentToFix}
        >
          <IconX size={11} /> {counts.errors} error{counts.errors === 1 ? "" : "s"}
        </button>
      ) : null}
    </div>
  );

  const panel = (
    <div className="flex h-full min-h-0 flex-col">
      <ErrorBoundary what="The chat">
      <Chat
        transcript={app.transcript}
        agentRunning={app.agentRunning}
        selection={app.selection}
        draft={draft}
        sessionInfo={app.sessionInfo}
        sessions={app.sessions}
        onSend={(t) => void app.sendMessage(t)}
        onRespondPermission={(id, allow) => void app.respondPermission(id, allow)}
        onInterrupt={app.interruptAgent}
        onNewSession={() => void app.newSession()}
        onClearSelection={() => app.setSelection([])}
        onResumeSession={(id) => void app.resumeSession(id)}
      />
      </ErrorBoundary>
    </div>
  );

  return (
    <Shell
      macTrafficLights={macOverlayChrome}
      titlebar={titlebar}
      rightSidebar={hasBoard ? panel : undefined}
      shellApiRef={shellApiRef}
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
            <Palette
              refreshKey={app.display.view?.source_hash ?? null}
              projectOpen={app.project !== null}
              armed={placement}
              onArm={armPlacement}
              armedLabel={labelMode}
              onArmLabel={armLabel}
            />
            <ErrorBoundary what="The canvas">
            <CircuitCanvas
              view={app.display.view}
              source={source}
              dimmed={app.display.dimmed}
              diagnostics={app.diagnostics}
              selection={app.selection}
              onSelectionChange={app.setSelection}
              onSavePositions={app.savePositions}
              placement={placement}
              onPlacementCommit={commitPlacement}
              onPlacementFinish={() => setPlacement(null)}
              labelMode={labelMode}
              onLabelFinish={() => setLabelMode(null)}
              onAttachPin={commitAttach}
              onRenameNet={commitRenameNet}
              onConnectPins={commitConnect}
              onUndo={async () => {
                const label = await app.undoGesture();
                // An undo may have reverted a placement — drop optimistic
                // stand-ins; the rebuild redraws whatever is still real.
                setProvisionals([]);
                return label;
              }}
              onRedo={async () => {
                const label = await app.redoGesture();
                setProvisionals([]);
                return label;
              }}
              onSetAttribute={commitSetAttribute}
              onRenameInstance={commitRenameInstance}
              onDisconnectPin={async (path, pin) => {
                await disconnectPin(path, pin, baseHash ?? "");
              }}
              onRemoveInstances={commitRemove}
              onAskAgent={(text) => void app.sendMessage(text)}
              latestHash={baseHash ?? null}
              provisionals={provisionals}
              onProvisionalMoved={(name, x, y) =>
                setProvisionals((prev) =>
                  prev.map((p) => (p.name === name ? { ...p, x, y } : p)),
                )
              }
            />
            </ErrorBoundary>
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
