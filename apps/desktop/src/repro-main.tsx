// Dev-only: mount Chat with stub props (see repro.html).
import React from "react";
import ReactDOM from "react-dom/client";
import { Shell } from "@etchable/ui";
import Chat from "./chat/Chat";
import type { ChatItem } from "./chat/messages";
import CircuitCanvas from "./circuit/CircuitCanvas";
import type { BuildView } from "./types";
import demoCircuit from "./repro-fixtures/demo-circuit.json";
import "./App.css";

// ?state=turn renders a finished turn with thinking + tools + answer.
const TURN: ChatItem[] = [
  { kind: "user", id: 1, text: "What does this board do?" },
  {
    kind: "thinking",
    id: 2,
    text: "The user wants an overview. Let me read the schematic first, then trace the power path from the USB connector.",
    streaming: false,
  },
  { kind: "agent", id: 3, text: "I'll look at the schematic to find out.", streaming: false },
  {
    kind: "tool",
    id: 4,
    toolUseId: "t1",
    name: "mcp__etchable__get_schematic",
    input: { scope: "full" },
    result: { content: "{ …17 components… }", isError: false },
  },
  {
    kind: "thinking",
    id: 5,
    text: "A CH340C and an AMS1117 — this is a USB-UART adapter.",
    streaming: false,
  },
  {
    kind: "tool",
    id: 6,
    toolUseId: "t2",
    name: "ToolSearch",
    input: { query: "select:TaskCreate,TaskUpdate", max_results: 5 },
    result: { content: "2 tools loaded", isError: false },
  },
  {
    kind: "tool",
    id: 7,
    toolUseId: "t3",
    name: "TaskCreate",
    input: {
      subject: "Read the schematic",
      description: "Understand the board",
      activeForm: "Reading the schematic",
    },
    result: { content: "Task #1 created successfully: Read the schematic", isError: false },
  },
  {
    kind: "tool",
    id: 8,
    toolUseId: "t4",
    name: "TaskCreate",
    input: {
      subject: "Add the status LED",
      description: "Wire an LED",
      activeForm: "Adding the status LED",
    },
    result: { content: "Task #2 created successfully: Add the status LED", isError: false },
  },
  {
    kind: "tool",
    id: 9,
    toolUseId: "t5",
    name: "TaskUpdate",
    input: { taskId: "1", status: "completed" },
    result: { content: "Updated task #1 status", isError: false },
  },
  {
    kind: "tool",
    id: 10,
    toolUseId: "t6",
    name: "TaskUpdate",
    input: { taskId: "2", status: "in_progress" },
    result: { content: "Updated task #2 status", isError: false },
  },
  {
    kind: "tool",
    id: 11,
    toolUseId: "t7",
    name: "Edit",
    input: {
      file_path: "/Users/rose/Desktop/Demo/board.zen",
      old_string: "line1\nline2",
      new_string: "line1\nline2\nline3",
    },
    result: { content: "ok", isError: false },
  },
  {
    kind: "tool",
    id: 12,
    toolUseId: "t8",
    name: "Bash",
    input: { command: "cargo test -p zen-build" },
    result: { content: "error: test failed", isError: true },
  },
  { kind: "agent", id: 13, text: "This is a **USB-C to serial adapter** built around the CH340C.", streaming: false },
  { kind: "result", id: 14, isError: false, subtype: "success", costUsd: 0.12, numTurns: 2, durationMs: 8300, at: Date.now() },
];

// ?state=working renders a just-sent message with the agent silent.
const WORKING: ChatItem[] = [{ kind: "user", id: 1, text: "Add a status LED" }];

// ?state=tools renders a turn with a tool currently in flight.
const TOOLS: ChatItem[] = [
  { kind: "user", id: 1, text: "Add a status LED" },
  { kind: "agent", id: 2, text: "Let me look at the board first.", streaming: false },
  {
    kind: "tool",
    id: 3,
    toolUseId: "t1",
    name: "mcp__etchable__get_diagnostics",
    input: {},
    result: { content: "no errors", isError: false },
  },
  {
    kind: "tool",
    id: 4,
    toolUseId: "t2",
    name: "mcp__etchable__get_schematic",
    input: { scope: "full" },
  },
];

// ?state=permission renders all three approval outcomes: an allowed one
// (should vanish, leaving only the executed row), a denied one (struck
// record), and a pending one (Allow/Deny card).
const PERMISSION: ChatItem[] = [
  { kind: "user", id: 1, text: "Search the web and run the tests" },
  {
    kind: "permission",
    id: 2,
    requestId: "p1",
    toolName: "WebSearch",
    input: { query: "CH340C vs CH340G differences" },
    verdict: "allowed",
  },
  {
    kind: "tool",
    id: 3,
    toolUseId: "t1",
    name: "WebSearch",
    input: { query: "CH340C vs CH340G differences" },
    result: { content: "5 results", isError: false },
  },
  {
    kind: "permission",
    id: 4,
    requestId: "p2",
    toolName: "WebFetch",
    input: { url: "https://example.com/datasheet" },
    verdict: "denied",
  },
  // Pending pair: the tool_use (running) + the permission — must render as
  // ONE approval card, not a row plus a card.
  {
    kind: "tool",
    id: 5,
    toolUseId: "t2",
    name: "Bash",
    input: { command: "cargo test --workspace" },
  },
  {
    kind: "permission",
    id: 6,
    requestId: "p3",
    toolName: "Bash",
    input: { command: "cargo test --workspace" },
  },
];

// ?state=canvas renders the schematic canvas with the demo board's circuit
// json (regenerate src/repro-fixtures/demo-circuit.json with
// `cargo run -q -p zen-build -- examples/demo/board.zen --circuit-json`).
// Exercises camera lock across container resizes.
function CanvasRepro() {
  const view: BuildView = {
    version: 3,
    source: "examples/demo/board.zen",
    schematic: null,
    diagnostics: [],
    circuit_json: (demoCircuit as { elements: BuildView["circuit_json"] }).elements,
    id_map: (demoCircuit as { id_map: Record<string, string> }).id_map,
    source_hash: null,
  };
  const [selection, setSelection] = React.useState<string[]>([]);
  return (
    <CircuitCanvas
      view={view}
      source={view.source}
      dimmed={false}
      diagnostics={[]}
      selection={selection}
      onSelectionChange={setSelection}
      onSavePositions={(p) => console.log("savePositions", p)}
    />
  );
}

// ?state=shell renders the app shell with both sidebars; used to verify a
// user-closed sidebar stays closed across window resizes.
function ShellRepro() {
  return (
    <Shell
      titlebar={<div className="px-3 text-sm">shell repro</div>}
      leftSidebar={<div className="p-3 text-sm">left sidebar</div>}
      rightSidebar={<div className="p-3 text-sm" data-testid="right-sidebar">right sidebar</div>}
      rightMinWidth={340}
      defaultRightWidth={360}
    >
      <div className="dotgrid h-full w-full p-4 text-sm">content</div>
    </Shell>
  );
}

function Repro() {
  const state = new URLSearchParams(location.search).get("state");
  if (state === "canvas") {
    return (
      <div style={{ position: "fixed", inset: 0, display: "flex" }}>
        <CanvasRepro />
      </div>
    );
  }
  if (state === "shell") {
    return (
      <div style={{ position: "fixed", inset: 0 }}>
        <ShellRepro />
      </div>
    );
  }
  const transcripts: Record<string, ChatItem[]> = {
    turn: TURN,
    working: WORKING,
    tools: TOOLS,
    permission: PERMISSION,
  };
  return (
    <Chat
      transcript={transcripts[state ?? ""] ?? []}
      agentRunning={state === "working" || state === "tools" || state === "permission"}
      sessions={[]}
      onResumeSession={() => {}}
      selection={[]}
      sessionInfo={{ model: "claude-fable-5" }}
      onSend={(t) => console.log("send", t)}
      onRespondPermission={() => {}}
      onInterrupt={() => {}}
      onNewSession={() => {}}
      onClearSelection={() => {}}
    />
  );
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <Repro />
  </React.StrictMode>,
);
