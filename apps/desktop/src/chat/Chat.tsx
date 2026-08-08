import { useState } from "react";
import {
  AssistantRuntimeProvider,
  ComposerPrimitive,
  MessagePrimitive,
  ThreadPrimitive,
  type MessageState,
  type ReasoningMessagePartProps,
  type TextMessagePartProps,
  type ToolCallMessagePartProps,
} from "@assistant-ui/react";
import { MarkdownTextPrimitive } from "@assistant-ui/react-markdown";
import remarkGfm from "remark-gfm";
import {
  Button,
  IconChevronDown,
  IconChevronRight,
  IconCrosshair,
  IconPlus,
  IconStop,
  IconX,
  Spinner,
} from "@etchable/ui";
import type { SessionInfo } from "../state";
import { formatResultFooter, prettyJson, previewInput, type ChatItem } from "./messages";
import { turnResultOf, useChatRuntime, type TurnResult } from "./runtime";

const MCP_PREFIX = "mcp__etchable__";

const CODE_BLOCK =
  "m-0 max-h-[200px] select-text overflow-auto whitespace-pre-wrap wrap-anywhere rounded-md bg-white px-2 py-1.5 font-mono text-[10px] shadow-[inset_0_0_0_0.5px_rgba(35,43,63,0.05)]";

type ChatProps = {
  transcript: ChatItem[];
  agentRunning: boolean;
  selection: string[];
  sessionInfo: SessionInfo | null;
  onSend: (text: string) => void;
  onRespondPermission: (requestId: string, allow: boolean) => void;
  onInterrupt: () => void;
  onNewSession: () => void;
  onClearSelection: () => void;
};

// ---- message parts ---------------------------------------------------------

function UserText({ text }: TextMessagePartProps) {
  return <>{text}</>;
}

function AssistantText({ status }: TextMessagePartProps) {
  return (
    <>
      <MarkdownTextPrimitive remarkPlugins={[remarkGfm]} className="chat-md" />
      {status.type === "running" && <span className="caret-blink">▍</span>}
    </>
  );
}

function ToolRow({ toolName, args, result, isError }: ToolCallMessagePartProps) {
  const [open, setOpen] = useState(false);
  const isMcp = toolName.startsWith(MCP_PREFIX);
  const displayName = isMcp ? toolName.slice(MCP_PREFIX.length) : toolName;
  const resultText = typeof result === "string" ? result : prettyJson(result);
  return (
    <div className="overflow-hidden rounded-lg bg-elev">
      <button
        type="button"
        className="flex w-full min-w-0 cursor-pointer items-center gap-[7px] px-2 py-[5px] text-left text-xxs transition-colors hover:bg-ink/4"
        onClick={() => setOpen(!open)}
      >
        <span className="flex flex-none text-ink/35">
          {open ? <IconChevronDown size={11} /> : <IconChevronRight size={11} />}
        </span>
        <span className="flex-none font-mono font-semibold">{displayName}</span>
        {isMcp && (
          <span className="flex-none rounded-full border border-sky/40 px-[5px] font-mono text-[9px] text-sky">
            canvas
          </span>
        )}
        <span className="min-w-0 flex-1 truncate font-mono text-[10px] text-ink/35">
          {previewInput(args)}
        </span>
        {result === undefined && <Spinner />}
        {isError && (
          <span className="flex flex-none text-alert">
            <IconX size={11} />
          </span>
        )}
      </button>
      {open && (
        <div className="flex flex-col gap-1.5 border-t border-ink/5 px-2 py-[7px]">
          <pre className={`${CODE_BLOCK} text-ink/55`}>{prettyJson(args)}</pre>
          {result !== undefined && (
            <pre className={`${CODE_BLOCK} ${isError ? "text-alert" : "text-ink/92"}`}>
              {resultText || "(empty result)"}
            </pre>
          )}
        </div>
      )}
    </div>
  );
}

function PermissionCard(props: ToolCallMessagePartProps) {
  const { toolName, args, approval, respondToApproval } = props;
  if (!approval) return null;
  const answered = approval.approved !== undefined;
  return (
    <div className="flex flex-col gap-[7px] rounded-[10px] bg-warn/5 px-2.5 py-2 ring-1 ring-warn/35">
      <div className="text-xs">
        Claude wants to use <strong className="font-mono">{toolName}</strong>
      </div>
      <pre className={`${CODE_BLOCK} text-ink/55`}>{prettyJson(args)}</pre>
      <div className="flex items-center gap-2">
        <Button
          size="sm"
          tone="success"
          disabled={answered}
          onClick={() => respondToApproval({ approved: true })}
        >
          Allow
        </Button>
        <Button
          size="sm"
          tone="danger"
          disabled={answered}
          onClick={() => respondToApproval({ approved: false })}
        >
          Deny
        </Button>
        {answered ? (
          <span
            className={
              "font-mono text-[10.5px] " +
              (approval.approved ? "text-leaf-deep" : "text-alert")
            }
          >
            {approval.approved ? "Allowed" : "Denied"}
          </span>
        ) : (
          <span className="animate-pulse text-[10px] text-ink/35">waiting for you</span>
        )}
      </div>
    </div>
  );
}

/** Tool-call parts carrying an approval are permission prompts; the rest are tool rows. */
function ToolCallPart(props: ToolCallMessagePartProps) {
  if (props.approval) return <PermissionCard {...props} />;
  return <ToolRow {...props} />;
}

function ThinkingPart({ text, status }: ReasoningMessagePartProps) {
  const [open, setOpen] = useState(false);
  if (status.type === "running") {
    // Live: clip to roughly the last three lines, newest visible.
    return (
      <div className="flex">
        <div className="flex max-h-[52px] max-w-[95%] flex-col justify-end overflow-hidden text-xxs italic text-ink/45">
          <div className="whitespace-pre-wrap wrap-anywhere">
            {text}
            <span className="caret-blink">▍</span>
          </div>
        </div>
      </div>
    );
  }
  return (
    <div className="flex flex-col">
      <button
        type="button"
        className="inline-flex w-fit cursor-pointer items-center gap-1 text-xxs italic text-ink/35 transition-colors hover:text-ink/55"
        onClick={() => setOpen((v) => !v)}
      >
        {open ? <IconChevronDown size={10} /> : <IconChevronRight size={10} />}
        Thought
      </button>
      {open && (
        <div className="mt-1 max-w-[95%] select-text whitespace-pre-wrap wrap-anywhere text-xxs italic text-ink/45">
          {text}
        </div>
      )}
    </div>
  );
}

// ---- messages --------------------------------------------------------------

function UserMessage() {
  return (
    <MessagePrimitive.Root className="flex justify-end">
      <div className="max-w-[85%] select-text whitespace-pre-wrap wrap-anywhere rounded-xl rounded-br-[4px] border border-copper/25 bg-copper/8 px-2.5 py-[7px] text-xs">
        <MessagePrimitive.Parts components={{ Text: UserText }} />
      </div>
    </MessagePrimitive.Root>
  );
}

function AssistantMessage() {
  return (
    <MessagePrimitive.Root className="flex">
      <div className="min-w-0 max-w-[95%] select-text wrap-anywhere text-xs">
        <MessagePrimitive.Parts
          components={{ Text: AssistantText, Reasoning: ThinkingPart }}
        />
      </div>
    </MessagePrimitive.Root>
  );
}

function ToolMessage() {
  return (
    <MessagePrimitive.Root>
      <MessagePrimitive.Parts components={{ tools: { Fallback: ToolCallPart } }} />
    </MessagePrimitive.Root>
  );
}

function SystemRow({ message }: { message: MessageState }) {
  const part = message.content[0];
  const text = part?.type === "text" ? part.text : "";
  const isError =
    (message.metadata?.custom as { isError?: boolean } | undefined)?.isError ?? false;
  return (
    <div
      className={
        "select-text whitespace-pre-wrap wrap-anywhere rounded-r-md border-l-2 px-2.5 py-1.5 font-mono text-[10.5px] " +
        (isError ? "border-alert bg-alert/5 text-alert" : "border-ink/8 bg-elev text-ink/55")
      }
    >
      {text}
    </div>
  );
}

function ResultRow({ result }: { result: TurnResult }) {
  const meta = formatResultFooter(result);
  return (
    <div className="border-b border-ink/5 pb-1.5 pt-0.5 font-mono text-[10px] text-ink/35">
      {result.isError ? (
        <span className="text-alert">{result.subtype}</span>
      ) : (
        <span>{meta || "done"}</span>
      )}
      {result.isError && meta && <span> · {meta}</span>}
    </div>
  );
}

// ---- composer ---------------------------------------------------------------

function Composer({
  selection,
  sessionInfo,
  onNewSession,
  onClearSelection,
}: Pick<ChatProps, "selection" | "sessionInfo" | "onNewSession" | "onClearSelection">) {
  const selPreview = (() => {
    if (selection.length === 0) return "";
    const joined = selection.join(", ");
    return joined.length > 48 ? joined.slice(0, 48) + "…" : joined;
  })();

  return (
    <ComposerPrimitive.Root className="mx-2.5 mb-2.5 mt-1.5 flex flex-none flex-col gap-[7px] rounded-[14px] bg-white p-2 shadow-island">
      {selection.length > 0 && (
        <div className="flex min-w-0 items-center gap-1.5 rounded-full border border-sky/40 bg-sky/10 px-2.5 py-1 text-xxs">
          <span className="flex flex-none text-sky">
            <IconCrosshair size={12} />
          </span>
          <span className="min-w-0 flex-1 truncate">
            {selection.length} selected — included as context
            <span className="font-mono text-[10px] text-ink/55"> · {selPreview}</span>
          </span>
          <button
            type="button"
            className="flex flex-none cursor-pointer text-ink/55 hover:text-ink/92"
            title="Clear selection"
            onClick={onClearSelection}
          >
            <IconX size={11} />
          </button>
        </div>
      )}
      <div className="flex items-end gap-2">
        <ComposerPrimitive.Input
          minRows={3}
          maxRows={8}
          placeholder="Message Claude…"
          className="flex-1 select-text resize-none px-1 py-1 text-xs leading-relaxed outline-none placeholder:text-ink/35"
        />
        <ComposerPrimitive.Send asChild>
          <Button variant="copper" size="sm">
            Send
          </Button>
        </ComposerPrimitive.Send>
      </div>
      <div className="flex min-h-[22px] items-center gap-2">
        <ThreadPrimitive.If running>
          <ComposerPrimitive.Cancel asChild>
            <Button size="sm" tone="danger">
              <IconStop size={10} />
              Stop
            </Button>
          </ComposerPrimitive.Cancel>
        </ThreadPrimitive.If>
        <ThreadPrimitive.If running={false}>
          <span className="text-[10px] text-ink/35">
            Enter to send · Shift+Enter for newline
          </span>
        </ThreadPrimitive.If>
        <span className="flex-1" />
        {sessionInfo?.model && (
          <span className="font-mono text-[9.5px] text-ink/35">{sessionInfo.model}</span>
        )}
        <Button
          variant="ghost"
          size="sm"
          className="px-2"
          title="New session"
          onClick={onNewSession}
        >
          <IconPlus size={13} />
        </Button>
      </div>
    </ComposerPrimitive.Root>
  );
}

// ---- chat -------------------------------------------------------------------

export default function Chat(props: ChatProps) {
  const runtime = useChatRuntime({
    transcript: props.transcript,
    agentRunning: props.agentRunning,
    onSend: props.onSend,
    onRespondPermission: props.onRespondPermission,
    onInterrupt: props.onInterrupt,
  });

  // Liveness: cover every silent gap (session spawn, pre-first-token,
  // between tools). Streaming items and in-flight tool rows already show
  // their own activity; everything else gets the Working… row.
  const tail = props.transcript[props.transcript.length - 1];
  const showWorking =
    props.agentRunning &&
    (!tail ||
      tail.kind === "user" ||
      tail.kind === "system" ||
      (tail.kind === "tool" && !!tail.result) ||
      (tail.kind === "permission" && tail.verdict !== undefined) ||
      (tail.kind === "agent" && !tail.streaming) ||
      (tail.kind === "thinking" && !tail.streaming));

  return (
    <AssistantRuntimeProvider runtime={runtime}>
      <ThreadPrimitive.Root className="flex min-h-0 flex-1 flex-col">
        <ThreadPrimitive.Viewport
          autoScroll
          className="scroll-minimal flex min-h-0 flex-1 flex-col gap-2 overflow-y-auto px-3 pb-1.5 pt-3"
        >
          <ThreadPrimitive.Empty>
            <div className="m-auto max-w-[260px] text-center text-xs text-ink/35">
              Ask about the board — Claude can read the schematic and edit the source.
            </div>
          </ThreadPrimitive.Empty>
          <ThreadPrimitive.Messages>
            {({ message }) => {
              const turnResult = turnResultOf(message.metadata);
              if (turnResult) return <ResultRow result={turnResult} />;
              if (message.role === "user") return <UserMessage />;
              if (message.role === "system") return <SystemRow message={message} />;
              if (message.content[0]?.type === "tool-call") return <ToolMessage />;
              return <AssistantMessage />;
            }}
          </ThreadPrimitive.Messages>
          {showWorking && (
            <div className="flex items-center gap-2 text-xxs text-ink/35">
              <Spinner />
              Working…
            </div>
          )}
        </ThreadPrimitive.Viewport>
        <Composer
          selection={props.selection}
          sessionInfo={props.sessionInfo}
          onNewSession={props.onNewSession}
          onClearSelection={props.onClearSelection}
        />
      </ThreadPrimitive.Root>
    </AssistantRuntimeProvider>
  );
}
