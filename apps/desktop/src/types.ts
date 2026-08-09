// TypeScript mirrors of the Tauri backend contract. Field names are EXACT.

// ---- Storage DTOs (GENERATED from crates/store via ts-rs) ----------------
// Regenerate with `pnpm gen:store-types`; CI checks they're in sync.

export type { RecentProject } from "./generated/RecentProject";
export type { SessionSummary } from "./generated/SessionSummary";

// ---- Build data (snake_case, from "build-finished" event / get_state) ----

/// Bump in lockstep with apps/desktop/src-tauri/src/state.rs::BUILD_PAYLOAD_VERSION.
export const BUILD_PAYLOAD_VERSION = 4;

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
  /** Which instances/nets structured writers may target (decision 0009). */
  editability: EditabilityDoc | null;
};

export type EditabilityDoc = {
  instances: Record<string, InstanceEdit>; // key = dotted path (modules + components)
  nets: Record<string, NetEdit>; // key = net name
};

export type InstanceEdit = {
  editable: boolean;
  file?: string; // workspace-relative file of the creating call
  line?: number; // 1-based
  reason?: string; // why not editable
  anchor?: string; // nearest editable ancestor for refused instances
};

export type NetEdit = {
  editable: boolean;
  file?: string;
  line?: number;
  variable?: string; // the source variable the net is bound to
  reason?: string;
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

/** One partial move for save_positions: schematic-space center (y-up,
    circuit_json units); rotation omitted keeps authored/derived. Keys are
    instance paths. The backend merges into the save-all map. */
export type MoveIn = {
  x: number;
  y: number;
  rotation?: number;
  /** Added to the resolved base rotation (the rotate gesture). */
  rotate_by?: number;
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

// ---- Palette / placement (decision 0009 phase 1) ----

/// From get_palette (camelCase command payload).
export type PaletteGeneric = {
  name: string;
  /** The Module("…") spec add_instance takes. */
  spec: string;
  /** Refdes prefix for name suggestions (R, C, …). */
  prefix: string | null;
  params: string[];
};

export type PaletteComponent = {
  name: string;
  spec: string;
  description: string | null;
  lcsc: string | null;
};

export type PaletteView = {
  generics: PaletteGeneric[];
  components: PaletteComponent[];
};

/// A palette item armed for placement: the ghost follows the cursor until
/// the drop commits one add_instance write.
export type PlacementArm = {
  spec: string;
  label: string;
  prefix: string | null;
  /** Generics with a required value config get a value field in the drop form. */
  needsValue: boolean;
  /** Real outline for the aiming ghost, filled in by warm_placement. */
  ghost?: GhostGeometry | null;
};

/// A net-label tool armed from the palette: click a pin to attach it.
export type LabelArm = {
  kind: "Net" | "Power" | "Ground";
  /** Prefill for the name input ("" = derive from the clicked pin). */
  defaultName: string;
  label: string;
};

/// add_instance command result (snake_case, straight from the writer).
export type AddInstanceResult = {
  line: number;
  inserted: string;
  binding: string;
  position_key: string | null;
  /** Nets synthesized for required-but-unwired pins ({name}_{pin}). */
  placeholder_nets: string[];
  /** The instance's connection points (io names), for provisional rendering. */
  pins: string[];
  /** The part's real outline + pin offsets (schematic units, y-up). */
  ghost: GhostGeometry | null;
};

/// The real rendered shape of a part, from the placement preflight — ghosts
/// and stand-ins draw the true outline so the swap to the real symbol is
/// seamless.
export type GhostGeometry = {
  width: number;
  height: number;
  pins: { name: string; x: number; y: number }[];
};

/// connect_pins command result (tagged, snake_case from the writer).
export type ConnectOutcome =
  | {
      outcome: "applied";
      net: string;
      variable: string;
      created_def: boolean;
      already: boolean;
      merged_from: string | null;
      moved_refs: number;
      pruned_defs: string[];
      /** Set when an inner-pin endpoint resolved through a module port
          (e.g. "SENSE_DIV.VIN") — surfaced so the model teaches itself. */
      via_port?: string;
    }
  | { outcome: "needs_merge"; from: string; into: string; from_refs: number };

/// One hit from search_lcsc's `results` (crates/lcsc SearchHit).
export type LcscSearchHit = {
  lcsc: string;
  mpn: string;
  manufacturer: string;
  package: string;
  description: string;
  /** "basic" | "extended" — extended adds a JLC setup fee. */
  class: string;
  stock: number;
  min_qty: number;
  unit_price: number | null;
  datasheet: string | null;
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
  /** Only in replayed session history (load_session_history) — live user
      turns come from the composer, not the wire. */
  | { type: "user_text"; text: string }
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
