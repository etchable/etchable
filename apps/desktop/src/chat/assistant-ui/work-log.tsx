// t3code-style turn chrome, in etchable's skin: compact work-log rows for
// tool calls (icon · human heading · preview · status tick, expandable
// details), the "Working for Xs" pulse-dots timer, and the plan bar fed by
// the CLI's task system (TaskCreate/TaskUpdate, legacy TodoWrite).

import { useEffect, useRef, useState, type FC } from "react";
import {
  Check,
  CaretDown,
  Circuitry,
  CircleNotch,
  Crosshair,
  FileText,
  Globe,
  Hammer,
  ListChecks,
  MagnifyingGlass,
  Minus,
  PencilSimple,
  PuzzlePiece,
  Robot,
  Sparkle,
  Terminal,
  WarningCircle,
  Wrench,
  X,
  type Icon,
} from "@phosphor-icons/react";
import { useAuiState, type ToolCallMessagePartProps } from "@assistant-ui/react";
import { Button } from "../ui/button";
import { cn } from "../ui/utils";
import { MCP_PREFIX, humanFileTarget, previewInput, prettyJson } from "../messages";
import { turnPlanOf, type PlanStep } from "../runtime";

// ---- tool display -----------------------------------------------------------

type ToolDisplay = {
  icon: Icon;
  /** Past-tense human heading ("Edited", "Read schematic"). */
  heading: string;
  /** Present-progressive heading while the call is running ("Editing"). */
  headingActive?: string;
  /** Truncated one-liner next to the heading. */
  preview: string;
  /** The full thing being acted on (command/path/query) — shown by the
      approval card; JSON only as fallback. Work rows don't render it. */
  detail?: string;
  /** Consecutive calls of this tool collapse into ONE row: the constant
      heading plus each call's token joined ("Read up on parts · C1, C2"). */
  stack?: { heading: string; headingActive: string; token: string };
};

const str = (v: unknown): string | undefined =>
  typeof v === "string" && v.length > 0 ? v : undefined;

const basename = (p?: string): string | undefined => p?.split("/").pop();

/** The 19 canvas tools: icon + past/active headings + which arg previews. */
const MCP_DISPLAY: Record<
  string,
  { icon: Icon; heading: string; active?: string; arg?: string }
> = {
  get_board_state: {
    icon: Circuitry,
    heading: "Got oriented",
    active: "Getting oriented",
  },
  get_selection: { icon: Circuitry, heading: "Checked selection", active: "Checking selection" },
  get_schematic: { icon: Circuitry, heading: "Read schematic", active: "Reading schematic" },
  get_instance: { icon: Circuitry, heading: "Inspected", active: "Inspecting", arg: "path" },
  query_nets: { icon: Circuitry, heading: "Traced nets", active: "Tracing nets", arg: "net" },
  get_diagnostics: {
    icon: Circuitry,
    heading: "Checked diagnostics",
    active: "Checking diagnostics",
  },
  check_layout: {
    icon: Circuitry,
    heading: "Checked layout",
    active: "Checking layout",
    arg: "scope",
  },
  set_positions: {
    icon: PencilSimple,
    heading: "Arranged the canvas",
    active: "Arranging the canvas",
  },
  find_empty_space: {
    icon: Crosshair,
    heading: "Found open space",
    active: "Finding open space",
  },
  get_bom: { icon: MagnifyingGlass, heading: "Read the BOM", active: "Reading the BOM" },
  get_circuit_json: { icon: Circuitry, heading: "Read the canvas", active: "Reading the canvas" },
  build: { icon: Hammer, heading: "Rebuilt the board", active: "Rebuilding the board" },
  list_library: {
    icon: MagnifyingGlass,
    heading: "Browsed library",
    active: "Browsing library",
    arg: "query",
  },
  search_parts: {
    icon: MagnifyingGlass,
    heading: "Searched parts",
    active: "Searching parts",
    arg: "query",
  },
  get_part: {
    icon: MagnifyingGlass,
    heading: "Checked part",
    active: "Checking part",
    arg: "lcsc",
  },
  add_component: {
    icon: PuzzlePiece,
    heading: "Installed component",
    active: "Installing component",
    arg: "name",
  },
  install_component: {
    icon: PuzzlePiece,
    heading: "Installed component",
    active: "Installing component",
    arg: "name",
  },
  get_symbol_pins: {
    icon: FileText,
    heading: "Read symbol pins",
    active: "Reading symbol pins",
  },
  zener_reference: {
    icon: FileText,
    heading: "Read the Zener guide",
    active: "Reading the Zener guide",
  },
};

export function toolDisplay(toolName: string, args: unknown): ToolDisplay {
  const a = (args ?? {}) as Record<string, unknown>;

  if (toolName.startsWith(MCP_PREFIX)) {
    const name = toolName.slice(MCP_PREFIX.length);
    // Part-sourcing tools carry the part identity — bake it into the
    // heading so the row explains itself ("Installed the ch340c component").
    switch (name) {
      case "get_lcsc_part": {
        const c = str(a.lcsc) ?? "";
        return {
          icon: MagnifyingGlass,
          heading: "Read up on a part",
          headingActive: "Reading up on a part",
          preview: c,
          detail: prettyJson(args),
          stack: { heading: "Read up on parts", headingActive: "Reading up on parts", token: c },
        };
      }
      case "add_lcsc_component":
      case "add_component": {
        const n = str(a.name);
        return {
          icon: PuzzlePiece,
          heading: n ? `Installed the ${n} component` : "Installed a component",
          headingActive: n ? `Installing the ${n} component` : "Installing a component",
          preview: str(a.lcsc) ?? "",
          detail: prettyJson(args),
          stack: {
            heading: "Installed components",
            headingActive: "Installing components",
            token: n ?? str(a.lcsc) ?? "",
          },
        };
      }
      case "fetch_datasheet": {
        const n = str(a.component);
        return {
          icon: FileText,
          heading: n ? `Fetched the ${n} datasheet` : "Fetched a datasheet",
          headingActive: n ? `Fetching the ${n} datasheet` : "Fetching a datasheet",
          preview: "",
          detail: prettyJson(args),
          stack: {
            heading: "Fetched datasheets",
            headingActive: "Fetching datasheets",
            token: n ?? "",
          },
        };
      }
      case "search_parts": {
        const q = str(a.query) ?? "";
        return {
          icon: MagnifyingGlass,
          heading: "Searched parts",
          headingActive: "Searching parts",
          preview: q,
          detail: prettyJson(args),
          stack: { heading: "Searched parts", headingActive: "Searching parts", token: q },
        };
      }
    }
    const known = MCP_DISPLAY[name];
    if (known) {
      const preview = (known.arg && str(a[known.arg])) ?? "";
      return {
        icon: known.icon,
        heading: known.heading,
        headingActive: known.active,
        preview,
        detail: prettyJson(args),
      };
    }
    return { icon: Wrench, heading: name, preview: previewInput(args), detail: prettyJson(args) };
  }

  switch (toolName) {
    case "Read": {
      const target = humanFileTarget(a.file_path);
      const token = target ?? basename(str(a.file_path)) ?? "";
      return {
        icon: FileText,
        heading: target ? `Read ${target}` : "Read",
        headingActive: target ? `Reading ${target}` : "Reading",
        // Recognized project files speak the app's vocabulary — no
        // filenames (source is an implementation detail).
        preview: target ? "" : (basename(str(a.file_path)) ?? ""),
        detail: str(a.file_path),
        stack: { heading: "Read", headingActive: "Reading", token },
      };
    }
    case "Edit":
    case "MultiEdit": {
      const target = humanFileTarget(a.file_path);
      const token = target ?? basename(str(a.file_path)) ?? "";
      return {
        icon: PencilSimple,
        heading: target ? `Edited ${target}` : "Edited",
        headingActive: target ? `Editing ${target}` : "Editing",
        preview: target ? "" : (basename(str(a.file_path)) ?? ""),
        detail: str(a.file_path),
        stack: { heading: "Edited", headingActive: "Editing", token },
      };
    }
    case "Write":
    case "NotebookEdit": {
      const path = str(a.file_path) ?? str(a.notebook_path);
      const target = humanFileTarget(path);
      const token = target ?? basename(path) ?? "";
      return {
        icon: PencilSimple,
        heading: target ? `Wrote ${target}` : "Wrote",
        headingActive: target ? `Writing ${target}` : "Writing",
        preview: target ? "" : (basename(path) ?? ""),
        detail: path,
        stack: { heading: "Wrote", headingActive: "Writing", token },
      };
    }
    case "Bash":
      return {
        icon: Terminal,
        heading: "Ran",
        headingActive: "Running",
        preview: str(a.command) ?? "",
        detail: str(a.command),
      };
    case "Grep":
    case "Glob": {
      const q = str(a.pattern) ?? "";
      return {
        icon: MagnifyingGlass,
        heading: "Searched",
        headingActive: "Searching",
        preview: q,
        detail: q,
        stack: { heading: "Searched", headingActive: "Searching", token: q },
      };
    }
    case "WebFetch": {
      let host = str(a.url) ?? "";
      try {
        host = new URL(host).hostname;
      } catch {
        /* keep raw */
      }
      return {
        icon: Globe,
        heading: "Fetched",
        headingActive: "Fetching",
        preview: host,
        detail: str(a.url),
      };
    }
    case "WebSearch":
      return {
        icon: Globe,
        heading: "Searched the web",
        headingActive: "Searching the web",
        preview: str(a.query) ?? "",
        detail: str(a.query),
      };
    case "ToolSearch":
      return {
        icon: MagnifyingGlass,
        heading: "Searched for tools",
        preview: "",
        stack: { heading: "Searched for tools", headingActive: "Searching for tools", token: "" },
      };
    case "Task":
    case "Agent":
      return {
        icon: Robot,
        heading: "Delegated",
        preview: str(a.description) ?? "",
        detail: str(a.prompt),
      };
    case "Skill":
      return { icon: Sparkle, heading: "Used skill", preview: str(a.skill) ?? "" };
    case "AskUserQuestion":
      return { icon: ListChecks, heading: "Asked you", preview: "" };
    case "EnterPlanMode":
    case "ExitPlanMode":
      return { icon: ListChecks, heading: "Planned", preview: "" };
    default:
      return {
        icon: Wrench,
        heading: toolName,
        preview: previewInput(args),
        detail: prettyJson(args),
      };
  }
}

// ---- work-log rows ---------------------------------------------------------

type RowStatus = "running" | "failed" | "done" | "pending";

const partStatus = (p: {
  result?: unknown;
  isError?: boolean;
  status?: { type: string };
}): RowStatus => {
  if (p.status?.type === "running") return "running";
  if (p.isError === true || p.status?.type === "incomplete") return "failed";
  if (p.result !== undefined) return "done";
  return "pending";
};

/** The single-line work-row visual: icon · heading · mono preview · tick. */
const RowShell: FC<{
  icon: Icon;
  heading: string;
  preview: string;
  status: RowStatus;
}> = ({ icon: RowIcon, heading, preview, status }) => (
  <div
    data-slot="work-row"
    className="flex w-full min-w-0 max-w-full items-center gap-1.5 rounded-md px-1 py-[3px] text-xxs"
  >
    <RowIcon className="size-3.5 shrink-0 text-ink/40" />
    <span className="shrink-0 font-medium text-ink/75">{heading}</span>
    <span className="min-w-0 flex-1 truncate font-mono text-[10px] text-muted-foreground/70">
      {preview}
    </span>
    <span className="flex shrink-0 items-center gap-1 text-ink/35">
      {status === "running" ? (
        <CircleNotch className="size-3 animate-spin [animation-duration:0.6s]" />
      ) : status === "failed" ? (
        <X className="size-3 text-alert" weight="bold" />
      ) : status === "done" ? (
        <Check className="size-3" weight="bold" />
      ) : (
        <Minus className="size-3 text-ink/25" />
      )}
    </span>
  </div>
);

/** One tool call as a quiet single-line row. Deliberately not expandable —
    the row IS the whole story (heading + preview + status tick). */
export const WorkRow: FC<ToolCallMessagePartProps> = ({
  toolName,
  args,
  result,
  isError,
  status,
}) => {
  const d = toolDisplay(toolName, args);
  const s = partStatus({ result, isError, status });
  return (
    <RowShell
      icon={d.icon}
      heading={s === "running" ? (d.headingActive ?? d.heading) : d.heading}
      preview={d.preview}
      status={s}
    />
  );
};

// ---- run stacking ----------------------------------------------------------

// Bursts of the same tool ("Read up on parts" × 12) collapse into one row:
// the run's FIRST part renders the stacked line, the rest render nothing.
// Runs are maximal sequences of consecutive tool-call parts sharing a
// toolName, stack-enabled, and approval-free.

type RawPart = {
  type: string;
  toolName?: string;
  toolCallId?: string;
  args?: unknown;
  result?: unknown;
  isError?: boolean;
  status?: { type: string };
  approval?: { approved?: boolean; resolution?: unknown } | null;
};

const inRun = (p: RawPart | undefined): p is RawPart =>
  p != null &&
  p.type === "tool-call" &&
  p.approval == null &&
  p.toolName !== undefined &&
  toolDisplay(p.toolName, p.args).stack !== undefined;

const sameRun = (a: RawPart, b: RawPart | undefined): b is RawPart =>
  inRun(b) && b.toolName === a.toolName;

/** [start, end] inclusive bounds of the run containing index i (i must be
    in a run), else null. */
function runBounds(parts: readonly RawPart[], i: number): [number, number] | null {
  const me = parts[i];
  if (!me || !inRun(me)) return null;
  let start = i;
  while (start > 0 && sameRun(me, parts[start - 1])) start--;
  let end = i;
  while (end + 1 < parts.length && sameRun(me, parts[end + 1])) end++;
  return [start, end];
}

/** Work row that folds itself into its run: the lead renders one stacked
    line for the whole burst, followers render nothing. Also hides while a
    pending approval matches this call (the card is the single UI then). */
export const RunAwareWorkRow: FC<ToolCallMessagePartProps> = (props) => {
  const pendingHidden = useAuiState(
    (s) =>
      props.result === undefined &&
      s.message.parts.some((p) => isPendingApproval(p) && sameCall(p, props)),
  );
  // Selectors return primitives only (an object here would be an unstable
  // getSnapshot and loop React — see SystemMessage).
  const role = useAuiState((s): "hidden" | "solo" | "lead" => {
    const parts = s.message.parts as readonly RawPart[];
    const i = parts.findIndex(
      (p) => p.type === "tool-call" && p.toolCallId === props.toolCallId,
    );
    const bounds = i >= 0 ? runBounds(parts, i) : null;
    if (!bounds || bounds[0] === bounds[1]) return "solo";
    return i === bounds[0] ? "lead" : "hidden";
  });
  const tokens = useAuiState((s): string => {
    if (role !== "lead") return "";
    const parts = s.message.parts as readonly RawPart[];
    const i = parts.findIndex(
      (p) => p.type === "tool-call" && p.toolCallId === props.toolCallId,
    );
    const bounds = i >= 0 ? runBounds(parts, i) : null;
    if (!bounds) return "";
    const seen = new Set<string>();
    for (let k = bounds[0]; k <= bounds[1]; k++) {
      const p = parts[k];
      const t = p.toolName ? toolDisplay(p.toolName, p.args).stack?.token : undefined;
      if (t) seen.add(t);
    }
    return [...seen].join(", ");
  });
  const runStatus = useAuiState((s): RowStatus => {
    if (role !== "lead") return "pending";
    const parts = s.message.parts as readonly RawPart[];
    const i = parts.findIndex(
      (p) => p.type === "tool-call" && p.toolCallId === props.toolCallId,
    );
    const bounds = i >= 0 ? runBounds(parts, i) : null;
    if (!bounds) return "pending";
    const statuses: RowStatus[] = [];
    for (let k = bounds[0]; k <= bounds[1]; k++) statuses.push(partStatus(parts[k]));
    if (statuses.includes("running")) return "running";
    if (statuses.includes("failed")) return "failed";
    if (statuses.every((st) => st === "done")) return "done";
    return "pending";
  });

  if (pendingHidden || role === "hidden") return null;
  if (role === "solo") return <WorkRow {...props} />;

  const d = toolDisplay(props.toolName, props.args);
  const stack = d.stack;
  if (!stack) return <WorkRow {...props} />;
  return (
    <RowShell
      icon={d.icon}
      heading={runStatus === "running" ? stack.headingActive : stack.heading}
      preview={tokens}
      status={runStatus}
    />
  );
};

// ---- approvals -------------------------------------------------------------

// The CLI represents one gated call as TWO transcript parts: the tool_use
// (running, no result) and the can_use_tool permission. They pair up by
// tool name + identical args; the pair must render as ONE thing.
type PartLike = {
  type: string;
  toolName?: string;
  args?: unknown;
  result?: unknown;
  approval?: { approved?: boolean; resolution?: unknown } | null;
};

const sameCall = (a: PartLike, b: PartLike) =>
  a.toolName === b.toolName && JSON.stringify(a.args) === JSON.stringify(b.args);

export const isPendingApproval = (p: PartLike) =>
  p.type === "tool-call" &&
  p.approval != null &&
  p.approval.approved === undefined &&
  p.approval.resolution === undefined;

/** Pending permission, in work-row language: friendly heading + the exact
    thing being approved (full command/url), then Allow/Deny. No JSON. */
export const ApprovalRow: FC<ToolCallMessagePartProps> = ({
  toolName,
  args,
  respondToApproval,
}) => {
  const [submitted, setSubmitted] = useState(false);
  const d = toolDisplay(toolName, args);
  const RowIcon = d.icon;

  const respond = (approved: boolean) => {
    if (submitted || !respondToApproval) return;
    respondToApproval({ approved });
    setSubmitted(true);
  };

  // The full detail is what the user is approving; only fall back to args
  // JSON when there is no natural single-line representation.
  const detail = d.detail ?? prettyJson(args);

  return (
    <div
      data-slot="approval-row"
      className="my-0.5 flex min-w-0 flex-col gap-1.5 rounded-lg border border-warn/35 bg-warn/5 px-2 py-1.5"
    >
      <div className="flex min-w-0 items-center gap-1.5 text-xxs">
        <RowIcon className="size-3.5 shrink-0 text-ink/50" />
        <span className="shrink-0 font-medium text-ink/85">
          Allow {(d.headingActive ?? d.heading).toLowerCase()}?
        </span>
        <span className="min-w-0 flex-1 truncate font-mono text-[10px] text-muted-foreground/80">
          {d.preview}
        </span>
        <WarningCircle className="size-3.5 shrink-0 text-warn" />
      </div>
      {detail && detail !== d.preview && (
        <div className="select-text whitespace-pre-wrap wrap-anywhere ps-5 font-mono text-[10px] leading-relaxed text-ink/70">
          {detail}
        </div>
      )}
      <div className="flex items-center gap-2 ps-5">
        <Button size="sm" onClick={() => respond(true)} disabled={submitted}>
          Allow
        </Button>
        <Button size="sm" variant="outline" onClick={() => respond(false)} disabled={submitted}>
          Deny
        </Button>
        <span className="animate-pulse text-[10px] text-muted-foreground">waiting for you</span>
      </div>
    </div>
  );
};

/** An answered approval renders nothing when its executed/failed tool call
    is present (that row is the record); a denied one with no matching call
    keeps a struck-through line. */
export const AnsweredApprovalRow: FC<ToolCallMessagePartProps> = (part) => {
  const hasMatchingCall = useAuiState((s) =>
    s.message.parts.some(
      (p) => p.type === "tool-call" && p.approval == null && sameCall(p, part),
    ),
  );
  if (part.approval?.approved === false && !hasMatchingCall) {
    return <DeniedRow toolName={part.toolName} args={part.args} />;
  }
  return null;
};

/** The record of a denied permission — no tool call follows it, so this
    line is all that remains. */
export const DeniedRow: FC<{ toolName: string; args?: unknown }> = ({ toolName, args }) => {
  const d = toolDisplay(toolName, args);
  const RowIcon = d.icon;
  return (
    <div
      data-slot="work-row-denied"
      className="flex w-full min-w-0 items-center gap-1.5 rounded-md px-1 py-[3px] text-xxs opacity-75"
    >
      <RowIcon className="size-3.5 shrink-0 text-ink/40" />
      <span className="shrink-0 font-medium text-ink/60 line-through">{d.heading}</span>
      <span className="min-w-0 flex-1 truncate font-mono text-[10px] text-muted-foreground/60">
        {d.preview}
      </span>
      <span className="shrink-0 font-mono text-[10px] text-alert">denied</span>
      <X className="size-3 shrink-0 text-alert" weight="bold" />
    </div>
  );
};

// ---- working timer ---------------------------------------------------------

const formatElapsed = (ms: number): string => {
  const s = Math.max(0, Math.floor(ms / 1000));
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m ${s % 60}s`;
  return `${Math.floor(m / 60)}h ${m % 60}m`;
};

/** Self-ticking elapsed label (updates the DOM directly, no re-renders). */
export const WorkingTimer: FC<{ startedAt: number }> = ({ startedAt }) => {
  const ref = useRef<HTMLSpanElement>(null);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const tick = () => {
      el.textContent = formatElapsed(Date.now() - startedAt);
    };
    tick();
    const t = setInterval(tick, 1000);
    return () => clearInterval(t);
  }, [startedAt]);
  return <span ref={ref} className="tabular-nums" />;
};

export const PulseDots: FC = () => (
  <span className="inline-flex shrink-0 items-center gap-[3px]" aria-hidden>
    <span className="animate-status-pulse size-1 rounded-full bg-ink/30" />
    <span className="animate-status-pulse size-1 rounded-full bg-ink/30 [animation-delay:200ms]" />
    <span className="animate-status-pulse size-1 rounded-full bg-ink/30 [animation-delay:400ms]" />
  </span>
);

// ---- plan bar --------------------------------------------------------------

/** t3code's turn plan: a segment bar + current step, fed by the turn's plan
    snapshot (folded from TaskCreate/TaskUpdate/TodoWrite in the runtime).
    Expands to the full checklist. */
export const PlanRow: FC = () => {
  const plan = useAuiState((s) => turnPlanOf(s.message.metadata));
  const [open, setOpen] = useState(false);
  if (!plan || plan.length === 0) return null;

  const completed = plan.filter((t) => t.status === "completed").length;
  const current = plan.find((t) => t.status === "in_progress");
  const label = current
    ? (current.activeForm ?? current.label)
    : completed === plan.length
      ? "All steps complete"
      : (plan.find((t) => t.status === "pending")?.label ?? "");

  return (
    <div data-slot="plan-row" className="py-0.5">
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        className="flex w-full min-w-0 cursor-pointer items-center gap-2 rounded-md px-1 py-[3px] text-left text-xxs transition-colors hover:bg-ink/4"
      >
        <CaretDown
          className={cn(
            "size-3 shrink-0 text-ink/40 transition-transform duration-150",
            !open && "-rotate-90",
          )}
        />
        <span className="flex shrink-0 items-center gap-0.5">
          {plan.map((t, i) => (
            <span
              key={i}
              className={cn(
                "h-[3px] w-2.5 rounded-full",
                t.status === "completed"
                  ? "bg-leaf"
                  : t.status === "in_progress"
                    ? "bg-copper"
                    : "bg-ink/20",
              )}
            />
          ))}
        </span>
        <span className="min-w-0 truncate text-ink/80">{label}</span>
        {plan.length > 1 && (
          <span className="shrink-0 font-mono text-[10px] text-ink/40">
            {completed}/{plan.length}
          </span>
        )}
      </button>
      {open && (
        <div className="mt-0.5 flex flex-col gap-px ps-7">
          {plan.map((t: PlanStep, i) => (
            <div key={i} className="flex items-baseline gap-2 text-xxs">
              <span className="w-3 shrink-0 text-center font-mono text-[10px] text-ink/45">
                {t.status === "completed" ? "✓" : t.status === "in_progress" ? "●" : "○"}
              </span>
              <span
                className={cn(
                  "min-w-0",
                  t.status === "completed" ? "text-ink/45" : "text-ink/75",
                )}
              >
                {t.label}
              </span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
};
