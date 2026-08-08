import { useEffect, useRef, useState } from "react";
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
import {
  formatResultFooter,
  prettyJson,
  previewInput,
  type ChatItem,
} from "./messages";

const MCP_PREFIX = "mcp__etchable__";

type ToolItem = Extract<ChatItem, { kind: "tool" }>;
type PermissionItem = Extract<ChatItem, { kind: "permission" }>;

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

function ToolRow({ item }: { item: ToolItem }) {
  const [open, setOpen] = useState(false);
  const isMcp = item.name.startsWith(MCP_PREFIX);
  const displayName = isMcp ? item.name.slice(MCP_PREFIX.length) : item.name;
  const codeBlock =
    "m-0 max-h-[200px] select-text overflow-auto whitespace-pre-wrap wrap-anywhere rounded-md bg-white px-2 py-1.5 font-mono text-[10px] shadow-[inset_0_0_0_0.5px_rgba(35,43,63,0.05)]";
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
          {previewInput(item.input)}
        </span>
        {!item.result && <Spinner />}
        {item.result?.isError && (
          <span className="flex flex-none text-alert">
            <IconX size={11} />
          </span>
        )}
      </button>
      {open && (
        <div className="flex flex-col gap-1.5 border-t border-ink/5 px-2 py-[7px]">
          <pre className={`${codeBlock} text-ink/55`}>{prettyJson(item.input)}</pre>
          {item.result && (
            <pre className={`${codeBlock} ${item.result.isError ? "text-alert" : "text-ink/92"}`}>
              {item.result.content || "(empty result)"}
            </pre>
          )}
        </div>
      )}
    </div>
  );
}

function PermissionCard({
  item,
  onRespond,
}: {
  item: PermissionItem;
  onRespond: (requestId: string, allow: boolean) => void;
}) {
  const answered = item.verdict !== undefined;
  return (
    <div className="flex flex-col gap-[7px] rounded-[10px] bg-warn/5 px-2.5 py-2 ring-1 ring-warn/35">
      <div className="text-xs">
        Claude wants to use <strong className="font-mono">{item.toolName}</strong>
      </div>
      <pre className="m-0 max-h-[200px] select-text overflow-auto whitespace-pre-wrap wrap-anywhere rounded-md bg-white px-2 py-1.5 font-mono text-[10px] text-ink/55 shadow-[inset_0_0_0_0.5px_rgba(35,43,63,0.05)]">
        {prettyJson(item.input)}
      </pre>
      <div className="flex items-center gap-2">
        <Button
          size="sm"
          tone="success"
          disabled={answered}
          onClick={() => onRespond(item.requestId, true)}
        >
          Allow
        </Button>
        <Button
          size="sm"
          tone="danger"
          disabled={answered}
          onClick={() => onRespond(item.requestId, false)}
        >
          Deny
        </Button>
        {answered && (
          <span
            className={
              "font-mono text-[10.5px] " +
              (item.verdict === "allowed" ? "text-leaf-deep" : "text-alert")
            }
          >
            {item.verdict === "allowed" ? "Allowed" : "Denied"}
          </span>
        )}
      </div>
    </div>
  );
}

function renderItem(item: ChatItem, onRespond: (id: string, allow: boolean) => void) {
  switch (item.kind) {
    case "user":
      return (
        <div key={item.id} className="flex justify-end">
          <div className="max-w-[85%] select-text whitespace-pre-wrap wrap-anywhere rounded-xl rounded-br-[4px] border border-copper/25 bg-copper/8 px-2.5 py-[7px] text-xs">
            {item.text}
          </div>
        </div>
      );
    case "agent":
      return (
        <div key={item.id} className="flex">
          <div className="max-w-[95%] select-text whitespace-pre-wrap wrap-anywhere text-xs">
            {item.text}
            {item.streaming && <span className="caret-blink">▍</span>}
          </div>
        </div>
      );
    case "tool":
      return <ToolRow key={item.id} item={item} />;
    case "permission":
      return <PermissionCard key={item.id} item={item} onRespond={onRespond} />;
    case "system":
      return (
        <div
          key={item.id}
          className={
            "select-text whitespace-pre-wrap wrap-anywhere rounded-r-md border-l-2 px-2.5 py-1.5 font-mono text-[10.5px] " +
            (item.isError
              ? "border-alert bg-alert/5 text-alert"
              : "border-ink/8 bg-elev text-ink/55")
          }
        >
          {item.text}
        </div>
      );
    case "result": {
      const meta = formatResultFooter(item);
      return (
        <div
          key={item.id}
          className="border-b border-ink/5 pb-1.5 pt-0.5 font-mono text-[10px] text-ink/35"
        >
          {item.isError ? <span className="text-alert">{item.subtype}</span> : <span>{meta || "done"}</span>}
          {item.isError && meta && <span> · {meta}</span>}
        </div>
      );
    }
  }
}

export default function Chat(props: ChatProps) {
  const {
    transcript,
    agentRunning,
    selection,
    sessionInfo,
    onSend,
    onRespondPermission,
    onInterrupt,
    onNewSession,
    onClearSelection,
  } = props;

  const [draft, setDraft] = useState("");
  const listRef = useRef<HTMLDivElement>(null);
  const stickRef = useRef(true);

  useEffect(() => {
    const el = listRef.current;
    if (el && stickRef.current) el.scrollTop = el.scrollHeight;
  }, [transcript]);

  const handleScroll = (e: React.UIEvent<HTMLDivElement>) => {
    const el = e.currentTarget;
    stickRef.current = el.scrollHeight - el.scrollTop - el.clientHeight < 80;
  };

  const send = () => {
    const text = draft.trim();
    if (!text) return;
    setDraft("");
    stickRef.current = true;
    onSend(text);
  };

  const selPreview = (() => {
    if (selection.length === 0) return "";
    const joined = selection.join(", ");
    return joined.length > 48 ? joined.slice(0, 48) + "…" : joined;
  })();

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div
        className="scroll-minimal flex min-h-0 flex-1 flex-col gap-2 overflow-y-auto px-3 pb-1.5 pt-3"
        ref={listRef}
        onScroll={handleScroll}
      >
        {transcript.length === 0 && (
          <div className="m-auto max-w-[260px] text-center text-xs text-ink/35">
            Ask about the board — Claude can read the schematic and edit the source.
          </div>
        )}
        {transcript.map((item) => renderItem(item, onRespondPermission))}
      </div>

      <div className="mx-2.5 mb-2.5 mt-1.5 flex flex-none flex-col gap-[7px] rounded-[14px] bg-white p-2 shadow-island">
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
          <textarea
            className="flex-1 select-text resize-none px-1 py-1 text-xs leading-relaxed outline-none placeholder:text-ink/35"
            placeholder="Message Claude…"
            value={draft}
            rows={3}
            onChange={(e) => setDraft(e.currentTarget.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.shiftKey) {
                e.preventDefault();
                send();
              }
            }}
          />
          <Button
            variant="copper"
            size="sm"
            onClick={send}
            disabled={draft.trim().length === 0}
          >
            Send
          </Button>
        </div>
        <div className="flex min-h-[22px] items-center gap-2">
          {agentRunning ? (
            <Button size="sm" tone="danger" onClick={onInterrupt}>
              <IconStop size={10} />
              Stop
            </Button>
          ) : (
            <span className="text-[10px] text-ink/35">
              Enter to send · Shift+Enter for newline
            </span>
          )}
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
      </div>
    </div>
  );
}
