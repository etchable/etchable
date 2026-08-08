// assistant-ui external-store bridge: the backend-driven ChatItem transcript
// stays the source of truth; this converts it into assistant-ui's message
// model and maps composer/approval callbacks back onto the Tauri commands.

import {
  useExternalStoreRuntime,
  type AppendMessage,
  type AssistantRuntime,
  type ThreadMessageLike,
} from "@assistant-ui/react";
import type { ChatItem } from "./messages";

/** Turn-footer payload smuggled through message metadata (see convertChatItem). */
export type TurnResult = {
  isError: boolean;
  subtype: string;
  costUsd?: number;
  numTurns?: number;
  durationMs?: number;
};

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

export function convertChatItem(item: ChatItem): ThreadMessageLike {
  const id = String(item.id);
  switch (item.kind) {
    case "user":
      return { role: "user", id, content: [{ type: "text", text: item.text }] };
    case "agent":
      return {
        role: "assistant",
        id,
        content: [{ type: "text", text: item.text }],
        status: item.streaming
          ? { type: "running" }
          : { type: "complete", reason: "stop" },
      };
    case "thinking":
      return {
        role: "assistant",
        id,
        content: [
          {
            type: "reasoning",
            text: item.text,
            status: item.streaming ? { type: "running" } : { type: "complete" },
          },
        ],
        status: item.streaming
          ? { type: "running" }
          : { type: "complete", reason: "stop" },
      };
    case "tool":
      return {
        role: "assistant",
        id,
        content: [
          {
            type: "tool-call",
            toolCallId: item.toolUseId,
            toolName: item.name,
            args: asArgs(item.input),
            ...(item.result
              ? { result: item.result.content, isError: item.result.isError }
              : {}),
          },
        ],
      };
    case "permission":
      // Permission prompts ride assistant-ui's tool-approval channel: the
      // part renders as an approval card until `approved` is set, and the
      // response comes back through onRespondToToolApproval below.
      return {
        role: "assistant",
        id,
        content: [
          {
            type: "tool-call",
            toolCallId: `permission-${item.requestId}`,
            toolName: item.toolName,
            args: asArgs(item.input),
            approval: {
              id: item.requestId,
              ...(item.verdict !== undefined
                ? { approved: item.verdict === "allowed" }
                : {}),
            },
          },
        ],
      };
    case "system":
      return {
        role: "system",
        id,
        content: [{ type: "text", text: item.text }],
        metadata: { custom: { isError: item.isError } },
      };
    case "result":
      // No renderable parts — the footer row renders from metadata alone.
      return {
        role: "assistant",
        id,
        content: [],
        metadata: {
          custom: {
            turnResult: {
              isError: item.isError,
              subtype: item.subtype,
              costUsd: item.costUsd,
              numTurns: item.numTurns,
              durationMs: item.durationMs,
            } satisfies TurnResult,
          },
        },
      };
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
  return useExternalStoreRuntime<ChatItem>({
    messages: transcript,
    isRunning: agentRunning,
    convertMessage: convertChatItem,
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
