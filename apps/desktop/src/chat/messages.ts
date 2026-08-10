// Chat transcript model: agent events fold into a list of renderable items.

import type { AgentEvent } from "../types";

export type ChatItem =
  | { kind: "user"; id: number; text: string; at?: number }
  | { kind: "agent"; id: number; text: string; streaming: boolean }
  | { kind: "thinking"; id: number; text: string; streaming: boolean }
  | {
      kind: "tool";
      id: number;
      toolUseId: string;
      name: string;
      input: unknown;
      result?: { content: string; isError: boolean };
    }
  | {
      kind: "permission";
      id: number;
      requestId: string;
      toolName: string;
      input: unknown;
      verdict?: "allowed" | "denied";
    }
  | { kind: "system"; id: number; text: string; isError: boolean }
  | {
      kind: "result";
      id: number;
      isError: boolean;
      subtype: string;
      result?: string;
      costUsd?: number;
      numTurns?: number;
      durationMs?: number;
      at?: number;
    };

export type TranscriptAction =
  | { type: "agent-event"; event: AgentEvent }
  | { type: "user"; text: string }
  | { type: "system"; text: string; isError?: boolean }
  | { type: "permission-answered"; requestId: string; allow: boolean }
  | { type: "clear" };

let idCounter = 1;
function nextId(): number {
  return idCounter++;
}

/** Index (from the end) of the currently-streaming agent draft, or -1. */
function findDraft(items: ChatItem[]): number {
  for (let i = items.length - 1; i >= 0; i--) {
    const it = items[i];
    if (it.kind === "agent" && it.streaming) return i;
    if (it.kind === "user" || it.kind === "result") break;
  }
  return -1;
}

/** Index of the streaming thinking draft. Same break set as findDraft:
    within a message, deltas arrive thinking → text, so the complete
    `thinking` event must scan past the agent draft and tool rows. */
function findThinkingDraft(items: ChatItem[]): number {
  for (let i = items.length - 1; i >= 0; i--) {
    const it = items[i];
    if (it.kind === "thinking" && it.streaming) return i;
    if (it.kind === "user" || it.kind === "result") break;
  }
  return -1;
}

/** Close a streaming thinking draft, if any. */
function closeThinking(items: ChatItem[]): ChatItem[] {
  const i = findThinkingDraft(items);
  if (i < 0) return items;
  const next = items.slice();
  next[i] = { ...(next[i] as ChatItem & { kind: "thinking" }), streaming: false };
  return next;
}

export function transcriptReducer(items: ChatItem[], action: TranscriptAction): ChatItem[] {
  switch (action.type) {
    case "clear":
      return [];
    case "user":
      return [...items, { kind: "user", id: nextId(), text: action.text, at: Date.now() }];
    case "system":
      return [
        ...items,
        { kind: "system", id: nextId(), text: action.text, isError: action.isError ?? false },
      ];
    case "permission-answered": {
      return items.map((it) =>
        it.kind === "permission" && it.requestId === action.requestId
          ? { ...it, verdict: action.allow ? "allowed" : "denied" }
          : it,
      );
    }
    case "agent-event":
      return applyAgentEvent(items, action.event);
  }
}

function applyAgentEvent(items: ChatItem[], ev: AgentEvent): ChatItem[] {
  switch (ev.type) {
    case "thinking_delta": {
      const i = findThinkingDraft(items);
      if (i >= 0) {
        const draft = items[i] as ChatItem & { kind: "thinking" };
        const next = items.slice();
        next[i] = { ...draft, text: draft.text + ev.text };
        return next;
      }
      return [...items, { kind: "thinking", id: nextId(), text: ev.text, streaming: true }];
    }
    case "thinking": {
      // Complete block replaces the streamed draft; if deltas were lagged
      // away, append a closed block so the thought still shows.
      const i = findThinkingDraft(items);
      if (i >= 0) {
        const next = items.slice();
        next[i] = { kind: "thinking", id: items[i].id, text: ev.text, streaming: false };
        return next;
      }
      return [...items, { kind: "thinking", id: nextId(), text: ev.text, streaming: false }];
    }
    case "stream_delta": {
      // Prose starting means the thought is over — close it first (also
      // prevents two thinking blocks in one message from merging).
      const closed = closeThinking(items);
      const i = findDraft(closed);
      if (i >= 0) {
        const draft = closed[i] as ChatItem & { kind: "agent" };
        const next = closed.slice();
        next[i] = { ...draft, text: draft.text + ev.text };
        return next;
      }
      return [...closed, { kind: "agent", id: nextId(), text: ev.text, streaming: true }];
    }
    case "assistant_text": {
      // Final text REPLACES the streaming draft (if any).
      const i = findDraft(items);
      if (i >= 0) {
        const next = items.slice();
        next[i] = { kind: "agent", id: items[i].id, text: ev.text, streaming: false };
        return next;
      }
      return [...items, { kind: "agent", id: nextId(), text: ev.text, streaming: false }];
    }
    case "tool_use":
      return [
        ...items,
        { kind: "tool", id: nextId(), toolUseId: ev.id, name: ev.name, input: ev.input },
      ];
    case "tool_result": {
      for (let i = items.length - 1; i >= 0; i--) {
        const it = items[i];
        if (it.kind === "tool" && it.toolUseId === ev.toolUseId) {
          const next = items.slice();
          next[i] = { ...it, result: { content: ev.content, isError: ev.isError } };
          return next;
        }
      }
      return items;
    }
    case "permission_request":
      return [
        ...items,
        {
          kind: "permission",
          id: nextId(),
          requestId: ev.requestId,
          toolName: ev.toolName,
          input: ev.input,
        },
      ];
    case "result": {
      // Close out any dangling drafts (text and thinking), then append the
      // turn footer.
      let next = closeThinking(items);
      const i = findDraft(next);
      if (i >= 0) {
        next = next === items ? items.slice() : next;
        const draft = next[i] as ChatItem & { kind: "agent" };
        next[i] = { ...draft, streaming: false };
      }
      return [
        ...next,
        {
          kind: "result",
          id: nextId(),
          isError: ev.isError,
          subtype: ev.subtype,
          result: ev.result,
          costUsd: ev.costUsd,
          numTurns: ev.numTurns,
          durationMs: ev.durationMs,
          at: Date.now(),
        },
      ];
    }
    // init / status / control_request / raw don't create transcript rows.
    default:
      return items;
  }
}

// ---- display helpers -------------------------------------------------------

export const MCP_PREFIX = "mcp__etchable__";

/** The 19 canvas tools (docs/decisions/0003+0004+0006), as live activities. */
const MCP_ACTIVITY: Record<string, string> = {
  get_board_state: "Getting oriented…",
  get_selection: "Checking the selection…",
  get_schematic: "Reading the schematic…",
  get_instance: "Inspecting a component…",
  query_nets: "Tracing nets…",
  get_diagnostics: "Checking diagnostics…",
  check_layout: "Checking the layout…",
  set_positions: "Arranging the canvas…",
  find_empty_space: "Finding open space…",
  get_bom: "Reading the BOM…",
  get_circuit_json: "Reading the canvas…",
  build: "Rebuilding…",
  list_library: "Browsing the library…",
  search_parts: "Searching parts…",
  get_part: "Checking a part…",
  add_component: "Installing a component…",
  get_symbol_pins: "Reading symbol pins…",
  fetch_datasheet: "Fetching a datasheet…",
  zener_reference: "Reading the language guide…",
};

function fileBasename(input: unknown): string | null {
  if (input && typeof input === "object" && !Array.isArray(input)) {
    const p = (input as Record<string, unknown>).file_path;
    if (typeof p === "string" && p.length > 0) return p.split("/").pop() ?? null;
  }
  return null;
}

/** Human name for a project file the agent touches — users think in boards,
    components, and part cards, not files (docs/product.md: source is an
    implementation detail). Null for files with no project vocabulary. */
export function humanFileTarget(path: unknown): string | null {
  if (typeof path !== "string" || path.length === 0) return null;
  const parts = path.split("/").filter(Boolean);
  const base = parts[parts.length - 1] ?? "";
  const dir = parts[parts.length - 2] ?? "";
  const stem = base.replace(/\.[^.]+$/, "");
  if (base === "etch.toml") return "project info";
  if (base === "pcb.toml") return "the project manifest";
  if (base.endsWith(".zen")) {
    if (dir === "components") return `the ${stem} component`;
    if (stem === "board") return "the board";
    return `the ${stem} module`;
  }
  if (base.endsWith(".toml") && dir === "components") return `${stem}'s part card`;
  if (dir === "datasheets") return `the ${stem} datasheet`;
  return null;
}

/** "What is it doing right now" — the running-tool status line. */
export function activityLabel(toolName: string, input?: unknown): string {
  if (toolName.startsWith(MCP_PREFIX)) {
    const name = toolName.slice(MCP_PREFIX.length);
    return MCP_ACTIVITY[name] ?? `Using ${name}…`;
  }
  const filePath =
    input && typeof input === "object" ? (input as Record<string, unknown>).file_path : undefined;
  const file = humanFileTarget(filePath) ?? fileBasename(input);
  switch (toolName) {
    case "Read":
      return file ? `Reading ${file}…` : "Reading files…";
    case "Edit":
    case "Write":
      return file ? `Editing ${file}…` : "Editing the board…";
    case "Bash":
      return "Running a command…";
    case "Grep":
    case "Glob":
      return "Searching the workspace…";
    case "WebFetch":
    case "WebSearch":
      return "Browsing the web…";
    case "ToolSearch":
      return "Finding tools…";
    case "TodoWrite":
    case "TaskCreate":
    case "TaskUpdate":
    case "EnterPlanMode":
    case "ExitPlanMode":
      return "Planning…";
    case "Task":
    case "Agent":
      return "Delegating…";
    case "Skill":
      return "Using a skill…";
    default:
      return `Using ${toolName}…`;
  }
}

/** One-line preview of a tool input (file_path / command / first chars of JSON). */
export function previewInput(input: unknown): string {
  if (input && typeof input === "object" && !Array.isArray(input)) {
    const obj = input as Record<string, unknown>;
    for (const key of ["file_path", "path", "command", "pattern", "url", "query", "text"]) {
      const v = obj[key];
      if (typeof v === "string" && v.length > 0) {
        return v.length > 60 ? v.slice(0, 60) + "…" : v;
      }
    }
  }
  let s: string;
  try {
    s = JSON.stringify(input) ?? "";
  } catch {
    s = String(input);
  }
  return s.length > 60 ? s.slice(0, 60) + "…" : s;
}

export function prettyJson(input: unknown): string {
  try {
    return JSON.stringify(input, null, 2) ?? String(input);
  } catch {
    return String(input);
  }
}

export function formatResultFooter(item: {
  durationMs?: number;
  costUsd?: number;
  numTurns?: number;
}): string {
  const parts: string[] = [];
  if (item.durationMs !== undefined) parts.push(`${(item.durationMs / 1000).toFixed(1)}s`);
  if (item.costUsd !== undefined) parts.push(`$${item.costUsd.toFixed(2)}`);
  if (item.numTurns !== undefined) {
    parts.push(`${item.numTurns} turn${item.numTurns === 1 ? "" : "s"}`);
  }
  return parts.join(" · ");
}
