// TypeScript mirrors of the Tauri backend contract. Field names are EXACT.

// ---- Storage DTOs (GENERATED from crates/store via ts-rs) ----------------
// Regenerate with `pnpm gen:store-types`; CI checks they're in sync.

export type { RecentProject } from "./generated/RecentProject";
export type { SessionSummary } from "./generated/SessionSummary";

// ---- Build data (snake_case, from "build-finished" event / get_state) ----

/// Bump in lockstep with apps/desktop/src-tauri/src/state.rs::BUILD_PAYLOAD_VERSION.
export const BUILD_PAYLOAD_VERSION = 3;

/// A Circuit JSON element (tscircuit's intermediary format). Kept opaque —
/// the UI only reads `type` and id fields; everything else belongs to the
/// viewer.
export type CircuitJsonElement = { type: string } & Record<string, unknown>;

export type BuildView = {
  version: number;
  source: string;
  schematic: SchematicDoc | null;
  diagnostics: Diag[];
  circuit_json: CircuitJsonElement[];
  /** Circuit JSON id -> instance path (or net name). Never parse ids apart. */
  id_map: Record<string, string>;
  /** SHA-256 of the board source at build time; save_positions' base_hash. */
  source_hash: string | null;
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
  position?: { x: number; y: number; rotation: number; mirror?: string | null };
};

/** Authored position payload for save_positions (keys = instance paths). */
export type PositionIn = {
  x: number;
  y: number;
  rotation: number;
  mirror?: string | null;
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

/// Project summary (camelCase, from get_state / "project-changed").
export type ProjectView = {
  name: string;
  root: string;
  board: string | null;
  problems: string[];
};

export type BackendState = {
  workspaceRoot: string | null;
  source: string | null;
  selection: { paths: string[]; note?: string };
  agentRunning: boolean;
  build: BuildView | null;
  project: ProjectView | null;
  /** Unanswered permission prompts (the agent's turn is blocked on these);
      re-materialized as cards when the webview (re)mounts. */
  pendingPermissions: { requestId: string; toolName: string; input: unknown }[];
};

// ---- Agent events (camelCase, flat tagged union) ----

export type AgentEvent =
  | { type: "init"; sessionId?: string; model?: string }
  | { type: "assistant_text"; text: string }
  | { type: "stream_delta"; text: string }
  | { type: "thinking"; text: string }
  | { type: "thinking_delta"; text: string }
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
