// Central app state: hydrates from get_state, subscribes to backend events,
// and exposes actions that wrap the Tauri commands.

import { useCallback, useEffect, useMemo, useReducer, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import type { UnlistenFn } from "@tauri-apps/api/event";
import type {
  AgentEvent,
  BackendState,
  BuildStartedPayload,
  BuildSummary,
  BuildView,
  Diag,
  MoveIn,
  ProjectView,
  SessionSummary,
} from "./types";
import { BUILD_PAYLOAD_VERSION } from "./types";
import { transcriptReducer, type ChatItem } from "./chat/messages";

export type SessionInfo = { sessionId?: string; model?: string };

export type Etchable = ReturnType<typeof useEtchable>;

const NO_DIAGS: Diag[] = [];

export function useEtchable() {
  const [source, setSource] = useState<string | null>(null);
  /**
   * Keep one spelling of the board path. `build-started` sends the absolute
   * path while `build-finished` sends it root-stripped (BuildView.source), so
   * naively storing both makes `source` flip on every build — which remounts
   * the canvas viewer (its React key) and flickers the titlebar. Same file,
   * same state: whoever names it first wins until the board actually changes.
   */
  const setSourceStable = (next: string | null) =>
    setSource((prev) => {
      if (prev === next || next === null) return prev ?? next;
      if (prev === null) return next;
      const same =
        prev === next || prev.endsWith(`/${next}`) || next.endsWith(`/${prev}`);
      return same ? prev : next;
    });
  const [build, setBuild] = useState<BuildView | null>(null);
  const [lastGood, setLastGood] = useState<BuildView | null>(null);
  const [building, setBuilding] = useState(false);
  const [boardError, setBoardError] = useState<string | null>(null);
  const [selection, setSelectionState] = useState<string[]>([]);
  const [agentRunning, setAgentRunning] = useState(false);
  const [project, setProject] = useState<ProjectView | null>(null);
  const [sessionInfo, setSessionInfo] = useState<SessionInfo | null>(null);
  const [transcript, dispatchTranscript] = useReducer(transcriptReducer, [] as ChatItem[]);
  const [sessions, setSessions] = useState<SessionSummary[]>([]);
  const workspaceRootRef = useRef<string | null>(null);

  // Resumable sessions for this window's workspace (the store hides
  // resume-superseded ancestors).
  const refreshSessions = useCallback((root: string | null) => {
    workspaceRootRef.current = root;
    if (!root) {
      setSessions([]);
      return;
    }
    void invoke<SessionSummary[]>("list_sessions", { workspaceRoot: root })
      .then(setSessions)
      .catch((err) => console.warn("list_sessions failed", err));
  }, []);

  // ---- hydrate + event subscriptions (StrictMode double-mount safe) --------

  useEffect(() => {
    let disposed = false;
    const unlistens: UnlistenFn[] = [];
    // Window-scoped on purpose: each project window is an independent
    // document, and the backend emits build/agent events to exactly one
    // window. A global listen would hear every window's events.
    const win = getCurrentWebviewWindow();
    const listen = win.listen.bind(win) as typeof win.listen;
    const track = (p: Promise<UnlistenFn>) => {
      p.then((u) => {
        if (disposed) u();
        else unlistens.push(u);
      }).catch((err) => console.error("listen failed", err));
    };

    // The backend and UI must agree on the build payload shape; a silent
    // mismatch would render garbage, so reject loudly instead.
    const acceptBuild = (view: BuildView): boolean => {
      if (view.version !== BUILD_PAYLOAD_VERSION) {
        const msg = `incompatible build payload: backend v${view.version}, UI v${BUILD_PAYLOAD_VERSION} — rebuild the app`;
        console.error(msg);
        setBoardError(msg);
        return false;
      }
      // Gesture commands nudge an immediate rebuild and the watcher echoes
      // ~150ms later with byte-identical results — keep the previous object
      // so nothing re-renders for the echo.
      const same = (prev: BuildView | null) =>
        prev !== null &&
        prev.source === view.source &&
        prev.source_hash !== null &&
        prev.source_hash === view.source_hash &&
        !!prev.schematic === !!view.schematic;
      setBuild((prev) => (same(prev) ? prev : view));
      if (view.schematic) setLastGood((prev) => (same(prev) ? prev : view));
      return true;
    };

    invoke<BackendState>("get_state")
      .then((s) => {
        if (disposed) return;
        setSourceStable(s.source);
        setSelectionState(s.selection?.paths ?? []);
        // Deliberately NOT hydrating agentRunning: the snapshot's flag means
        // "a session exists", not "a turn is in flight". A remounting UI has
        // no transcript to resume anyway; live agent-event status/result
        // updates own this bit. Mapping session-exists onto running left the
        // chat stuck on the working indicator after a webview reload.
        setProject(s.project ?? null);
        refreshSessions(s.workspaceRoot ?? null);
        if (s.build) acceptBuild(s.build);
        // …EXCEPT pending permissions: the CLI blocks its turn on them, so
        // losing the cards to a reload would wedge the session. Rebuild the
        // cards, and since a prompt implies a turn in flight, show running.
        const pending = s.pendingPermissions ?? [];
        for (const p of pending) {
          dispatchTranscript({
            type: "agent-event",
            event: {
              type: "permission_request",
              requestId: p.requestId,
              toolName: p.toolName,
              input: p.input,
            } as AgentEvent,
          });
        }
        if (pending.length > 0) setAgentRunning(true);
        // The dashboard's "Sketch it" flow queues the agent's first message
        // on the backend; pick it up exactly once and send it through the
        // normal path (user bubble + spawn included).
        void invoke<string | null>("take_initial_prompt")
          .then((prompt) => {
            if (!disposed && prompt) void sendMessage(prompt);
          })
          .catch(() => {});
      })
      .catch((err) => console.error("get_state failed", err));

    track(
      listen<BuildStartedPayload>("build-started", (e) => {
        setSourceStable(e.payload.source);
        setBuilding(true);
        setBoardError(null);
      }),
    );

    track(
      listen<ProjectView>("project-changed", (e) => {
        setProject(e.payload);
      }),
    );

    track(
      listen<BuildView>("build-finished", (e) => {
        setBuilding(false);
        setSourceStable(e.payload.source);
        acceptBuild(e.payload);
      }),
    );

    track(
      listen<AgentEvent>("agent-event", (e) => {
        const ev = e.payload;
        if (ev.type === "status") setAgentRunning(ev.running);
        else if (ev.type === "result") setAgentRunning(false);
        else if (ev.type === "init") {
          setSessionInfo({ sessionId: ev.sessionId, model: ev.model });
          // The backend just recorded this session — reflect it.
          refreshSessions(workspaceRootRef.current);
        }
        dispatchTranscript({ type: "agent-event", event: ev });
      }),
    );

    return () => {
      disposed = true;
      for (const u of unlistens) u();
    };
  }, []);

  // ---- actions --------------------------------------------------------------

  const openBoard = useCallback(async (path: string) => {
    setBoardError(null);
    setBuilding(true);
    setProject(null);
    try {
      await invoke<BuildSummary>("select_board", { path });
    } catch (err) {
      setBoardError(String(err));
      setBuilding(false);
    }
  }, []);

  const openProject = useCallback(async (path: string) => {
    setBoardError(null);
    setBuilding(true);
    try {
      await invoke<BuildSummary>("open_project", { path });
      const s = await invoke<BackendState>("get_state");
      setProject(s.project ?? null);
    } catch (err) {
      setBoardError(String(err));
      setBuilding(false);
    }
  }, []);

  const createProject = useCallback(async (parent: string, name: string) => {
    setBoardError(null);
    setBuilding(true);
    try {
      await invoke<BuildSummary>("create_project", { parent, name });
      const s = await invoke<BackendState>("get_state");
      setProject(s.project ?? null);
    } catch (err) {
      setBoardError(String(err));
      setBuilding(false);
    }
  }, []);

  const rebuild = useCallback(async () => {
    setBoardError(null);
    setBuilding(true);
    try {
      await invoke<BuildSummary>("rebuild");
    } catch (err) {
      setBoardError(String(err));
      setBuilding(false);
    }
  }, []);

  const setSelection = useCallback((paths: string[]) => {
    setSelectionState(paths);
    void invoke("set_selection", { paths }).catch(() => {
      /* fire-and-forget */
    });
  }, []);

  // Drag-to-move persistence: PARTIAL schematic-space moves; the backend
  // merges the save-all map (keeping derived orientations for the rest).
  // The watcher-triggered rebuild is the loop's confirmation; a "content
  // modified" rejection means the file changed under the edit (e.g. an agent
  // write) and that rebuild is already on its way — drop the stale edit.
  const savePositions = useCallback(
    (moves: Record<string, MoveIn>, baseHash: string) => {
      void invoke("save_positions", { moves, baseHash }).catch((err) => {
        console.warn("save_positions rejected:", err);
      });
    },
    [],
  );

  // Place one instance (decision 0009 phase 1): a single gated write of the
  // board file. Throws on rejection — the drop form shows the reason (a
  // "content modified" error means the board changed under the gesture; the
  // rebuild is already on its way, re-try against it).
  const addInstance = useCallback(
    async (
      module: string,
      name: string,
      attrs: [string, string][],
      position: { x: number; y: number; rotation: number } | null,
      baseHash: string,
    ): Promise<import("./types").AddInstanceResult> => {
      return await invoke("add_instance", { module, name, attrs, position, baseHash });
    },
    [],
  );

  // Attach a pin to a net (the label/rail gesture, phase 2). The backend
  // resolves the anchor call site from editability; throws with the reason.
  const disconnectPin = useCallback(
    async (instancePath: string, pin: string, baseHash: string) => {
      await invoke("disconnect_pin", { instancePath, pin, baseHash });
    },
    [],
  );

  const attachPinNet = useCallback(
    async (
      instancePath: string,
      pin: string,
      netName: string,
      kind: string,
      baseHash: string,
    ) => {
      await invoke("attach_pin_net", { instancePath, pin, netName, kind, baseHash });
    },
    [],
  );

  const renameNet = useCallback(async (from: string, to: string, baseHash: string) => {
    await invoke("rename_net", { from, to, baseHash });
  }, []);

  // Pre-warm a palette part's placement (pins + real geometry cached
  // backend-side; returns the outline for the aiming ghost).
  const warmPlacement = useCallback(
    async (spec: string): Promise<import("./types").GhostGeometry | null> => {
      try {
        return (await invoke("warm_placement", { spec })) ?? null;
      } catch {
        return null;
      }
    },
    [],
  );

  // Value/param edit (phase 4): one kwarg replaced as a string literal.
  const setAttribute = useCallback(
    async (instancePath: string, key: string, value: string, baseHash: string) => {
      await invoke("set_attribute", { instancePath, key, value, baseHash });
    },
    [],
  );

  const renameInstance = useCallback(
    async (from: string, to: string, baseHash: string) => {
      await invoke("rename_instance", { from, to, baseHash });
    },
    [],
  );

  // Delete the selection (batch, server resolves anchors + prunes orphans).
  const removeInstances = useCallback(
    async (instancePaths: string[], baseHash: string) => {
      await invoke("remove_instances", { instancePaths, baseHash });
    },
    [],
  );

  // Gesture undo/redo over the write gate's snapshots; resolves to the
  // gesture's label ("move", "connect_pins", …) for the toast.
  const undoGesture = useCallback(async () => {
    return await invoke<string>("undo_gesture");
  }, []);
  const redoGesture = useCallback(async () => {
    return await invoke<string>("redo_gesture");
  }, []);

  // Wire two pins (phase 3). needs_merge comes back as a value, not an
  // error — the canvas confirms and retries with allowMerge.
  const connectPins = useCallback(
    async (
      a: { path: string; pin: string },
      b: { path: string; pin: string },
      allowMerge: boolean,
      baseHash: string,
    ): Promise<import("./types").ConnectOutcome> => {
      return await invoke("connect_pins", {
        aPath: a.path,
        aPin: a.pin,
        bPath: b.path,
        bPin: b.pin,
        net: null,
        allowMerge,
        baseHash,
      });
    },
    [],
  );

  const sendMessage = useCallback(async (text: string) => {
    const trimmed = text.trim();
    if (!trimmed) return;
    dispatchTranscript({ type: "user", text: trimmed });
    // Optimistic: the first session spawn can take seconds, and the
    // liveness row keys off agentRunning.
    setAgentRunning(true);
    try {
      await invoke("send_message", { text: trimmed });
    } catch (err) {
      setAgentRunning(false);
      dispatchTranscript({ type: "system", text: String(err), isError: true });
    }
  }, []);

  const respondPermission = useCallback(async (requestId: string, allow: boolean) => {
    dispatchTranscript({ type: "permission-answered", requestId, allow });
    try {
      await invoke("respond_permission", { requestId, allow });
    } catch (err) {
      dispatchTranscript({ type: "system", text: String(err), isError: true });
    }
  }, []);

  const interruptAgent = useCallback(() => {
    void invoke("interrupt_agent").catch(() => {});
  }, []);

  // Loads the old conversation and arms `--resume` for the next send — the
  // CLI deliberately does NOT spawn (the user may only want to read).
  const resumeSession = useCallback(async (sessionId: string) => {
    dispatchTranscript({ type: "clear" });
    try {
      const events = await invoke<AgentEvent[]>("resume_session", { sessionId });
      for (const ev of events) {
        if (ev.type === "user_text") {
          dispatchTranscript({ type: "user", text: ev.text });
        } else {
          dispatchTranscript({ type: "agent-event", event: ev });
        }
      }
      // Marks the thread as attached to a session so the resume chip goes
      // away; the model name fills in from init when the CLI first spawns.
      setSessionInfo({ sessionId });
    } catch (err) {
      dispatchTranscript({ type: "system", text: String(err), isError: true });
    }
  }, []);

  const newSession = useCallback(async () => {
    try {
      await invoke("new_session");
      dispatchTranscript({ type: "clear" });
      setSessionInfo(null);
      // The session is dead; don't wait on its closing status event to
      // un-stick the working indicator.
      setAgentRunning(false);
    } catch (err) {
      dispatchTranscript({ type: "system", text: String(err), isError: true });
    }
  }, []);

  // ---- derived --------------------------------------------------------------

  const diagnostics = build?.diagnostics ?? NO_DIAGS;

  const counts = useMemo(() => {
    let errors = 0;
    let warnings = 0;
    let advice = 0;
    for (const d of diagnostics) {
      if (d.suppressed) continue;
      if (d.severity === "error") errors++;
      else if (d.severity === "warning") warnings++;
      else advice++;
    }
    let components = 0;
    if (build?.schematic) {
      for (const inst of Object.values(build.schematic.instances)) {
        if (inst.kind === "component") components++;
      }
    }
    return { errors, warnings, advice, components };
  }, [build, diagnostics]);

  // What the canvas should draw: the current build, or — when the latest
  // build is broken — the last good one for the same board, dimmed.
  const display = useMemo((): { view: BuildView | null; dimmed: boolean } => {
    if (!build) return { view: null, dimmed: false };
    if (build.schematic) return { view: build, dimmed: false };
    if (lastGood && lastGood.source === build.source) {
      return { view: lastGood, dimmed: true };
    }
    return { view: null, dimmed: true };
  }, [build, lastGood]);

  return {
    source,
    project,
    build,
    building,
    boardError,
    selection,
    agentRunning,
    sessionInfo,
    sessions,
    transcript,
    diagnostics,
    counts,
    display,
    openBoard,
    openProject,
    createProject,
    rebuild,
    setSelection,
    savePositions,
    addInstance,
    warmPlacement,
    attachPinNet,
    disconnectPin,
    renameNet,
    connectPins,
    setAttribute,
    renameInstance,
    removeInstances,
    undoGesture,
    redoGesture,
    sendMessage,
    respondPermission,
    interruptAgent,
    newSession,
    resumeSession,
  };
}
