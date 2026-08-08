// Central app state: hydrates from get_state, subscribes to backend events,
// and exposes actions that wrap the Tauri commands.

import { useCallback, useEffect, useMemo, useReducer, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  AgentEvent,
  BackendState,
  BuildOutput,
  BuildStartedPayload,
  BuildSummary,
  Diag,
  SchematicDoc,
} from "./types";
import { transcriptReducer, type ChatItem } from "./chat/messages";

export type SessionInfo = { sessionId?: string; model?: string };

export type Etchable = ReturnType<typeof useEtchable>;

const NO_DIAGS: Diag[] = [];

export function useEtchable() {
  const [source, setSource] = useState<string | null>(null);
  const [build, setBuild] = useState<BuildOutput | null>(null);
  const [lastGood, setLastGood] = useState<{ source: string; schematic: SchematicDoc } | null>(
    null,
  );
  const [building, setBuilding] = useState(false);
  const [boardError, setBoardError] = useState<string | null>(null);
  const [selection, setSelectionState] = useState<string[]>([]);
  const [agentRunning, setAgentRunning] = useState(false);
  const [sessionInfo, setSessionInfo] = useState<SessionInfo | null>(null);
  const [transcript, dispatchTranscript] = useReducer(transcriptReducer, [] as ChatItem[]);

  // ---- hydrate + event subscriptions (StrictMode double-mount safe) --------

  useEffect(() => {
    let disposed = false;
    const unlistens: UnlistenFn[] = [];
    const track = (p: Promise<UnlistenFn>) => {
      p.then((u) => {
        if (disposed) u();
        else unlistens.push(u);
      }).catch((err) => console.error("listen failed", err));
    };

    invoke<BackendState>("get_state")
      .then((s) => {
        if (disposed) return;
        setSource(s.source);
        setSelectionState(s.selection?.paths ?? []);
        setAgentRunning(s.agentRunning);
        if (s.build) {
          setBuild(s.build);
          if (s.build.schematic) {
            setLastGood({ source: s.build.source, schematic: s.build.schematic });
          }
        }
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
      listen<BuildOutput>("build-finished", (e) => {
        setBuilding(false);
        setSource(e.payload.source);
        setBuild(e.payload);
        if (e.payload.schematic) {
          setLastGood({ source: e.payload.source, schematic: e.payload.schematic });
        }
      }),
    );

    track(
      listen<AgentEvent>("agent-event", (e) => {
        const ev = e.payload;
        if (ev.type === "status") setAgentRunning(ev.running);
        else if (ev.type === "result") setAgentRunning(false);
        else if (ev.type === "init") {
          setSessionInfo({ sessionId: ev.sessionId, model: ev.model });
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
    try {
      await invoke<BuildSummary>("select_board", { path });
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

  const sendMessage = useCallback(async (text: string) => {
    const trimmed = text.trim();
    if (!trimmed) return;
    dispatchTranscript({ type: "user", text: trimmed });
    try {
      await invoke("send_message", { text: trimmed });
      setAgentRunning(true);
    } catch (err) {
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

  const newSession = useCallback(async () => {
    try {
      await invoke("new_session");
      dispatchTranscript({ type: "clear" });
      setSessionInfo(null);
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

  // What the canvas should draw: the current schematic, or — when the latest
  // build is broken — the last good one for the same board, dimmed.
  const display = useMemo((): { schematic: SchematicDoc | null; dimmed: boolean } => {
    if (!build) return { schematic: null, dimmed: false };
    if (build.schematic) return { schematic: build.schematic, dimmed: false };
    if (lastGood && lastGood.source === build.source) {
      return { schematic: lastGood.schematic, dimmed: true };
    }
    return { schematic: null, dimmed: true };
  }, [build, lastGood]);

  return {
    source,
    build,
    building,
    boardError,
    selection,
    agentRunning,
    sessionInfo,
    transcript,
    diagnostics,
    counts,
    display,
    openBoard,
    rebuild,
    setSelection,
    sendMessage,
    respondPermission,
    interruptAgent,
    newSession,
  };
}
