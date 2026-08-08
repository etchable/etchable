// TypeScript mirrors of the Tauri backend contract. Field names are EXACT.

// ---- Build data (snake_case, from "build-finished" event / get_state) ----

export type BuildOutput = {
  source: string;
  schematic: SchematicDoc | null;
  diagnostics: Diag[];
};

export type SchematicDoc = {
  root_module: string;
  instances: Record<string, InstanceDoc>; // key = dotted path, root key is "root"
  nets: Record<string, NetDoc>;
  by_refdes: Record<string, string>; // "R1" -> "root.SENSE_DIV.R1.R"
};

export type InstanceKind = "module" | "component" | "interface" | "port" | "pin";

export type PinDoc = { name: string; net?: string | null };

export type InstanceDoc = {
  path: string;
  kind: InstanceKind;
  type_name: string;
  source_file: string | null;
  refdes?: string;
  attributes?: Record<string, unknown>; // value, package, mpn, type, ...
  children?: Record<string, string>; // child name -> child path
  pins?: PinDoc[];
  position?: { x: number; y: number; rotation: number };
};

export type NetDoc = {
  name: string;
  kind: string;
  ports: { component: string; pin: string }[];
};

export type DiagSeverity = "error" | "warning" | "advice";

export type Diag = {
  severity: DiagSeverity;
  message: string;
  kind?: string;
  file?: string;
  line?: number;
  col?: number;
  suppressed?: boolean;
  stack?: string[];
};

// ---- Command results ----

export type BuildSummary = {
  source: string;
  ok: boolean;
  components: number;
  nets: number;
  errors: number;
  warnings: number;
};

export type BackendState = {
  workspaceRoot: string | null;
  source: string | null;
  selection: { paths: string[]; note?: string };
  agentRunning: boolean;
  build: BuildOutput | null;
};

// ---- Agent events (camelCase, flat tagged union) ----

export type AgentEvent =
  | { type: "init"; sessionId?: string; model?: string }
  | { type: "assistant_text"; text: string }
  | { type: "stream_delta"; text: string }
  | { type: "tool_use"; id: string; name: string; input: unknown }
  | { type: "tool_result"; toolUseId: string; content: string; isError: boolean }
  | { type: "permission_request"; requestId: string; toolName: string; input: unknown }
  | {
      type: "result";
      isError: boolean;
      subtype: string;
      result?: string;
      costUsd?: number;
      numTurns?: number;
      durationMs?: number;
    }
  | { type: "status"; running: boolean }
  | { type: "control_request"; requestId: string; request: unknown }
  | { type: "raw"; value: unknown };

export type BuildStartedPayload = { source: string };
