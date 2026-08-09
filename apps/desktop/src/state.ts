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
  PositionIn,
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
      setBuild(view);
      if (view.schematic) setLastGood(view);
      return true;
    };

    invoke<BackendState>("get_state")
      .then((s) => {
        if (disposed) return;
        setSource(s.source);
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
      })
      .catch((err) => console.error("get_state failed", err));

    track(
      listen<BuildStartedPayload>("build-started", (e) => {
        setSource(e.payload.source);
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
        setSource(e.payload.source);
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

  // Drag-to-move persistence: write authored positions into the board file.
  // The watcher-triggered rebuild is the loop's confirmation; a "content
  // modified" rejection means the file changed under the edit (e.g. an agent
  // write) and that rebuild is already on its way — drop the stale edit.
  const savePositions = useCallback(
    (positions: Record<string, PositionIn>, baseHash: string) => {
      void invoke("save_positions", { positions, baseHash }).catch((err) => {
        console.warn("save_positions rejected:", err);
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

  const resumeSession = useCallback(async (sessionId: string) => {
    dispatchTranscript({ type: "clear" });
    dispatchTranscript({ type: "system", text: "Resuming previous session…", isError: false });
    setAgentRunning(true);
    try {
      await invoke("resume_session", { sessionId });
    } catch (err) {
      setAgentRunning(false);
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
    sendMessage,
    respondPermission,
    interruptAgent,
    newSession,
    resumeSession,
  };
}
