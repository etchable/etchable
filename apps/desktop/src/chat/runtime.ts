// assistant-ui external-store bridge: the backend-driven ChatItem transcript
// stays the source of truth; this folds it into assistant-ui's message model
// (one message per agent TURN, so reasoning/tool parts group and the action
// bar appears once per turn) and maps composer/approval callbacks back onto
// the Tauri commands.

import { useMemo } from "react";
import {
  useExternalStoreRuntime,
  type AppendMessage,
  type AssistantRuntime,
  type ThreadMessageLike,
} from "@assistant-ui/react";
import type { ChatItem } from "./messages";

/** Turn-footer payload smuggled through message metadata. */
export type TurnResult = {
  isError: boolean;
  subtype: string;
  costUsd?: number;
  numTurns?: number;
  durationMs?: number;
  at?: number;
};

export function turnStartedAtOf(metadata: unknown): number | undefined {
  const custom = (metadata as { custom?: Record<string, unknown> } | undefined)?.custom;
  return custom?.startedAt as number | undefined;
}

/** One step of the agent's plan (CLI task system or legacy TodoWrite). */
export type PlanStep = {
  label: string;
  activeForm?: string;
  status: "pending" | "in_progress" | "completed";
};

export function turnPlanOf(metadata: unknown): PlanStep[] | undefined {
  const custom = (metadata as { custom?: Record<string, unknown> } | undefined)?.custom;
  return custom?.plan as PlanStep[] | undefined;
}

const PLAN_TOOLS = new Set(["TaskCreate", "TaskUpdate", "TodoWrite"]);

/** Fold the CLI's task tools into plan state. Task ids come back in the
    TaskCreate RESULT ("Task #3 created successfully: …"); creation order is
    the fallback, which matches the CLI's sequential ids. */
class PlanFold {
  private tasks = new Map<string, PlanStep>();
  private todos: PlanStep[] | null = null;
  private seq = 0;

  /** Returns true when the item changed plan state. */
  apply(item: ChatItem & { kind: "tool" }): boolean {
    const args = (item.input ?? {}) as Record<string, unknown>;
    switch (item.name) {
      case "TaskCreate": {
        this.seq += 1;
        const m =
          typeof item.result?.content === "string"
            ? /Task #(\w+)/.exec(item.result.content)
            : null;
        const id = m?.[1] ?? String(this.seq);
        this.tasks.set(id, {
          label:
            (typeof args.subject === "string" && args.subject) ||
            (typeof args.description === "string" && args.description) ||
            `Task #${id}`,
          ...(typeof args.activeForm === "string" ? { activeForm: args.activeForm } : {}),
          status: "pending",
        });
        return true;
      }
      case "TaskUpdate": {
        const id = String(args.taskId ?? "");
        const status = args.status;
        if (status === "deleted") {
          this.tasks.delete(id);
          return true;
        }
        const t = this.tasks.get(id);
        if (t && (status === "pending" || status === "in_progress" || status === "completed")) {
          t.status = status;
        }
        return true;
      }
      case "TodoWrite": {
        const todos = args.todos;
        if (Array.isArray(todos)) {
          this.todos = todos.map((t) => {
            const td = t as Record<string, unknown>;
            const status = td.status;
            return {
              label: typeof td.content === "string" ? td.content : "",
              ...(typeof td.activeForm === "string" ? { activeForm: td.activeForm } : {}),
              status:
                status === "in_progress" || status === "completed" ? status : "pending",
            } satisfies PlanStep;
          });
        }
        return true;
      }
      default:
        return false;
    }
  }

  snapshot(): PlanStep[] | undefined {
    if (this.tasks.size > 0) return [...this.tasks.values()].map((t) => ({ ...t }));
    if (this.todos) return this.todos.map((t) => ({ ...t }));
    return undefined;
  }
}

export function isPlanTool(name: string): boolean {
  return PLAN_TOOLS.has(name);
}

export function turnResultOf(metadata: unknown): TurnResult | undefined {
  const custom = (metadata as { custom?: Record<string, unknown> } | undefined)?.custom;
  return custom?.turnResult as TurnResult | undefined;
}

type Json = string | number | boolean | null | { [key: string]: Json } | Json[];

/** Tool inputs are JSON objects on the wire; wrap anything else defensively. */
function asArgs(input: unknown): Record<string, Json> {
  return input !== null && typeof input === "object" && !Array.isArray(input)
    ? (input as Record<string, Json>)
    : { value: input as Json };
}

type TurnPart = ChatItem & { kind: "agent" | "thinking" | "tool" | "permission" };

/** One assistant turn: the run of agent/thinking/tool/permission items
    between user/system boundaries, closed by a `result` item. */
type Turn = {
  kind: "turn";
  id: number;
  parts: TurnPart[];
  result?: ChatItem & { kind: "result" };
  running: boolean;
  /** When the triggering user message was sent (feeds the working timer). */
  startedAt?: number;
  /** Plan state as of this turn — only set on turns that touched it. */
  plan?: PlanStep[];
};

type StoreMessage = ChatItem | Turn;

/** Fold the flat transcript into user/system items + assistant turns. */
export function mergeTurns(items: ChatItem[], agentRunning: boolean): StoreMessage[] {
  const out: StoreMessage[] = [];
  let turn: Turn | null = null;
  let lastUserAt: number | undefined;
  const plan = new PlanFold();

  for (const item of items) {
    switch (item.kind) {
      case "user":
        lastUserAt = item.at;
        turn = null;
        out.push(item);
        break;
      case "system":
        turn = null;
        out.push(item);
        break;
      case "result":
        if (turn) {
          turn.result = item;
          turn = null;
        } else {
          out.push({
            kind: "turn",
            id: item.id,
            parts: [],
            result: item,
            running: false,
            startedAt: lastUserAt,
          });
        }
        break;
      default:
        if (!turn) {
          turn = { kind: "turn", id: item.id, parts: [], running: false, startedAt: lastUserAt };
          out.push(turn);
        }
        turn.parts.push(item);
        // Task/todo calls update the conversation-level plan; the turn
        // carries a snapshot so the plan bar shows current state.
        if (item.kind === "tool" && plan.apply(item)) {
          turn.plan = plan.snapshot();
        }
        break;
    }
  }

  // Only the trailing, unclosed turn can be live.
  const last = out[out.length - 1];
  if (agentRunning && last && "kind" in last && last.kind === "turn" && !last.result) {
    last.running = true;
  }
  return out;
}

function convertPart(item: TurnPart): Exclude<ThreadMessageLike["content"], string>[number] {
  switch (item.kind) {
    case "agent":
      return { type: "text", text: item.text };
    case "thinking":
      return {
        type: "reasoning",
        text: item.text,
        status: item.streaming ? { type: "running" } : { type: "complete" },
      };
    case "tool":
      return {
        type: "tool-call",
        toolCallId: item.toolUseId,
        toolName: item.name,
        args: asArgs(item.input),
        ...(item.result
          ? { result: item.result.content, isError: item.result.isError }
          : {}),
      };
    case "permission":
      // Permission prompts ride assistant-ui's tool-approval channel: the
      // part renders as an approval card until `approved` is set, and the
      // response comes back through onRespondToToolApproval below.
      return {
        type: "tool-call",
        toolCallId: `permission-${item.requestId}`,
        toolName: item.toolName,
        args: asArgs(item.input),
        approval: {
          id: item.requestId,
          ...(item.verdict !== undefined ? { approved: item.verdict === "allowed" } : {}),
        },
      };
  }
}

export function convertStoreMessage(message: StoreMessage): ThreadMessageLike {
  if (message.kind === "turn") {
    return {
      role: "assistant",
      id: String(message.id),
      content: message.parts.map(convertPart),
      status: message.running
        ? { type: "running" }
        : { type: "complete", reason: "stop" },
      metadata: {
        custom: {
          ...(message.startedAt !== undefined ? { startedAt: message.startedAt } : {}),
          ...(message.plan !== undefined ? { plan: message.plan } : {}),
          ...(message.result
            ? {
                turnResult: {
                  isError: message.result.isError,
                  subtype: message.result.subtype,
                  costUsd: message.result.costUsd,
                  numTurns: message.result.numTurns,
                  durationMs: message.result.durationMs,
                  at: message.result.at,
                } satisfies TurnResult,
              }
            : {}),
        },
      },
    };
  }
  const id = String(message.id);
  switch (message.kind) {
    case "user":
      return { role: "user", id, content: [{ type: "text", text: message.text }] };
    case "system":
      return {
        role: "system",
        id,
        content: [{ type: "text", text: message.text }],
        metadata: { custom: { isError: message.isError } },
      };
    default:
      // agent/thinking/tool/permission/result always live inside a Turn.
      throw new Error(`unmergeable chat item reached convert: ${message.kind}`);
  }
}

export type ChatRuntimeOptions = {
  transcript: ChatItem[];
  agentRunning: boolean;
  onSend: (text: string) => void;
  onRespondPermission: (requestId: string, allow: boolean) => void;
  onInterrupt: () => void;
};

export function useChatRuntime(opts: ChatRuntimeOptions): AssistantRuntime {
  const { transcript, agentRunning, onSend, onRespondPermission, onInterrupt } = opts;
  const messages = useMemo(
    () => mergeTurns(transcript, agentRunning),
    [transcript, agentRunning],
  );
  return useExternalStoreRuntime<StoreMessage>({
    messages,
    isRunning: agentRunning,
    convertMessage: convertStoreMessage,
    onNew: async (message: AppendMessage) => {
      const text = message.content
        .filter((part) => part.type === "text")
        .map((part) => part.text)
        .join("\n")
        .trim();
      if (text) onSend(text);
    },
    onCancel: async () => onInterrupt(),
    onRespondToToolApproval: ({ approvalId, approved }) => {
      onRespondPermission(approvalId, approved);
    },
  });
}
