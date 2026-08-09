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
    id: 15,
    toolUseId: "t9",
    name: "Edit",
    input: { file_path: "/Users/rose/Desktop/Demo/components/led.zen", old_string: "a", new_string: "b" },
    result: { content: "ok", isError: false },
  },
  {
    kind: "tool",
    id: 16,
    toolUseId: "t10",
    name: "Edit",
    input: { file_path: "/Users/rose/Desktop/Demo/etch.toml", old_string: "a", new_string: "b" },
    result: { content: "ok", isError: false },
  },
  {
    kind: "tool",
    id: 17,
    toolUseId: "t11",
    name: "Read",
    input: { file_path: "/Users/rose/Desktop/Demo/datasheets/ch340c.pdf" },
    result: { content: "(pdf)", isError: false },
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
    id: 20,
    toolUseId: "t20",
    name: "mcp__etchable__get_board_state",
    input: {},
    result: { content: "ok", isError: false },
  },
  {
    kind: "tool",
    id: 21,
    toolUseId: "t21",
    name: "mcp__etchable__check_layout",
    input: {},
    result: { content: "no overlaps", isError: false },
  },
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
// Pass &fixture=<url> (e.g. /@fs/abs/path.json via the vite dev server) to
// view any board's circuit json instead. Exercises camera lock across
// container resizes.
function CanvasRepro() {
  const fixtureUrl = new URLSearchParams(location.search).get("fixture");
  const [doc, setDoc] = React.useState<{
    elements: BuildView["circuit_json"];
    id_map: Record<string, string>;
  }>(demoCircuit as never);
  React.useEffect(() => {
    if (!fixtureUrl) return;
    void fetch(fixtureUrl)
      .then((r) => r.json())
      .then(setDoc)
      .catch((e) => console.error("fixture load failed", e));
  }, [fixtureUrl]);
  // A fake-but-shaped editability map + hash so selection-driven gestures
  // (Delete, R) exercise in the browser harness too.
  const fixtureEditability = React.useMemo(
    () => ({
      instances: Object.fromEntries(
        [...new Set(Object.values(doc.id_map))]
          .filter((p) => p.startsWith("root."))
          .map((p) => [p, { editable: true }]),
      ),
      nets: {},
    }),
    [doc],
  );
  const view: BuildView = {
    version: 4,
    source: fixtureUrl ?? "examples/demo/board.zen",
    schematic: null,
    diagnostics: [],
    circuit_json: doc.elements,
    id_map: doc.id_map,
    source_hash: "fixture-hash",
    editability: fixtureEditability,
  };
  const [selection, setSelection] = React.useState<string[]>([]);
  // ?state=canvas&place=1 arms a fake resistor placement (ghost + drop
  // form); &label=1 arms the GND label tool — console.log commits, no
  // Tauri backend in the repro page.
  const [placement, setPlacement] = React.useState(() =>
    new URLSearchParams(window.location.search).get("place")
      ? {
          spec: "@stdlib/generics/Resistor.zen",
          label: "Resistor",
          prefix: "R",
          needsValue: true,
        }
      : null,
  );
  const [labelMode, setLabelMode] = React.useState<
    import("./types").LabelArm | null
  >(() =>
    new URLSearchParams(window.location.search).get("label")
      ? { kind: "Ground", defaultName: "GND", label: "GND" }
      : null,
  );
  return (
    <CircuitCanvas
      view={view}
      source={view.source}
      dimmed={false}
      diagnostics={[]}
      selection={selection}
      onSelectionChange={setSelection}
      onSavePositions={(p) => console.log("savePositions", p)}
      placement={placement}
      onPlacementCommit={async (name, attrs, pos) => {
        console.log("addInstance", name, attrs, pos);
      }}
      onPlacementFinish={() => setPlacement(null)}
      labelMode={labelMode}
      onLabelFinish={() => setLabelMode(null)}
      onAttachPin={async (path, pin, netName, kind) => {
        console.log("attachPinNet", path, pin, netName, kind);
      }}
      onRenameNet={async (from, to) => {
        console.log("renameNet", from, to);
      }}
      onSetAttribute={async (path, key, value) => {
        console.log("setAttribute", path, key, value);
      }}
      onRenameInstance={async (from, to) => {
        console.log("renameInstance", from, to);
      }}
      onRemoveInstances={async (paths) => {
        console.log("removeInstances", paths);
      }}
      onAskAgent={(text) => console.log("askAgent", text)}
      latestHash="fixture-hash"
      provisionals={
        // ?state=canvas&prov=1 renders a fake pending part (dashed
        // stand-in with clickable pins) for visual/interaction smoke.
        new URLSearchParams(window.location.search).get("prov")
          ? [
              {
                name: "NT1",
                label: "NetTie",
                pins: ["P1", "P2"],
                x: -3,
                y: -2,
                rotation: 0,
                positionPath: "root.NT1.NT",
              },
            ]
          : []
      }
      onProvisionalMoved={(name, x, y) => console.log("provMoved", name, x, y)}
      onUndo={async () => {
        console.log("undo");
        return "move";
      }}
      onRedo={async () => {
        console.log("redo");
        return "move";
      }}
      onConnectPins={async (a, b, allowMerge) => {
        console.log("connectPins", a, b, allowMerge);
        // Exercise the merge-confirm card on the first attempt.
        if (!allowMerge) {
          return { outcome: "needs_merge", from: "NET_B", into: "NET_A", from_refs: 3 };
        }
        return {
          outcome: "applied",
          net: "NET_A",
          variable: "NET_A",
          created_def: false,
          already: false,
          merged_from: "NET_B",
          moved_refs: 3,
          pruned_defs: ["NET_B"],
        };
      }}
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

// ?state=resume mirrors the resume-last-session click: a system notice with
// the agent spinning up.
const RESUME: ChatItem[] = [
  { kind: "system", id: 1, text: "Resuming previous session…", isError: false },
];

// ?state=sourcing mirrors a part-sourcing burst (should stack to 3 rows).
const SOURCING: ChatItem[] = (() => {
  const items: ChatItem[] = [{ kind: "user", id: 1, text: "Source the parts" }];
  let id = 2;
  const check = (c: string) =>
    items.push({
      kind: "tool",
      id: id++,
      toolUseId: `t${id}`,
      name: "mcp__etchable__get_part",
      input: { lcsc: c },
      result: { content: "ok", isError: false },
    });
  const install = (name: string, lcsc: string) =>
    items.push({
      kind: "tool",
      id: id++,
      toolUseId: `t${id}`,
      name: "mcp__etchable__add_component",
      input: { name, lcsc },
      result: { content: "ok", isError: false },
    });
  ["C16581", "C2682616", "C165948", "C131337"].forEach(check);
  install("TP4056", "C16581");
  install("MAX17048", "C2682616");
  install("USB_C_Receptacle", "C165948");
  install("JST_PH_2", "C131337");
  ["C1591", "C19702", "C23186", "C21190", "C22975", "C25804", "C2286", "C72043"].forEach(check);
  return items;
})();

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
    resume: RESUME,
    sourcing: SOURCING,
  };
  return (
    <Chat
      transcript={transcripts[state ?? ""] ?? []}
      agentRunning={
        state === "working" || state === "tools" || state === "permission" || state === "resume"
      }
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
