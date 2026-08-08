import { useEffect, useRef, useState } from "react";
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
  return (
    <div className="tool-row">
      <button type="button" className="tool-head" onClick={() => setOpen(!open)}>
        <span className="tool-caret">{open ? "▾" : "▸"}</span>
        <span className="tool-name">{displayName}</span>
        {isMcp && <span className="tag-canvas">canvas</span>}
        <span className="tool-preview">{previewInput(item.input)}</span>
        {!item.result && <span className="spinner" aria-label="running" />}
        {item.result?.isError && <span className="tool-err-mark">✗</span>}
      </button>
      {open && (
        <div className="tool-body">
          <pre className="tool-json">{prettyJson(item.input)}</pre>
          {item.result && (
            <pre className={"tool-result" + (item.result.isError ? " is-error" : "")}>
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
    <div className="perm-card">
      <div className="perm-title">
        Claude wants to use <strong>{item.toolName}</strong>
      </div>
      <pre className="tool-json">{prettyJson(item.input)}</pre>
      <div className="perm-actions">
        <button
          type="button"
          className="btn btn-allow"
          disabled={answered}
          onClick={() => onRespond(item.requestId, true)}
        >
          Allow
        </button>
        <button
          type="button"
          className="btn btn-deny"
          disabled={answered}
          onClick={() => onRespond(item.requestId, false)}
        >
          Deny
        </button>
        {answered && (
          <span className={"perm-verdict " + (item.verdict === "allowed" ? "ok" : "no")}>
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
        <div key={item.id} className="msg-row right">
          <div className="bubble user">{item.text}</div>
        </div>
      );
    case "agent":
      return (
        <div key={item.id} className="msg-row">
          <div className={"agent-text" + (item.streaming ? " streaming" : "")}>
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
        <div key={item.id} className={"sys-row" + (item.isError ? " is-error" : "")}>
          {item.text}
        </div>
      );
    case "result": {
      const meta = formatResultFooter(item);
      return (
        <div key={item.id} className="result-row">
          {item.isError ? (
            <span className="result-err">{item.subtype}</span>
          ) : (
            <span>{meta || "done"}</span>
          )}
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
    <div className="chat">
      <div className="chat-list" ref={listRef} onScroll={handleScroll}>
        {transcript.length === 0 && (
          <div className="chat-placeholder">
            Ask about the board — Claude can read the schematic and edit the source.
          </div>
        )}
        {transcript.map((item) => renderItem(item, onRespondPermission))}
      </div>

      <div className="composer">
        {selection.length > 0 && (
          <div className="ctx-chip">
            <span className="ctx-chip-text">
              📌 {selection.length} selected — included as context
              <span className="ctx-chip-paths"> · {selPreview}</span>
            </span>
            <button
              type="button"
              className="ctx-chip-x"
              title="Clear selection"
              onClick={onClearSelection}
            >
              ×
            </button>
          </div>
        )}
        <div className="composer-main">
          <textarea
            className="composer-input"
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
          <button
            type="button"
            className="btn btn-send"
            onClick={send}
            disabled={draft.trim().length === 0}
          >
            Send
          </button>
        </div>
        <div className="composer-foot">
          {agentRunning ? (
            <button type="button" className="btn btn-stop" onClick={onInterrupt}>
              ■ Stop
            </button>
          ) : (
            <span className="composer-hint">Enter to send · Shift+Enter for newline</span>
          )}
          <span className="composer-spacer" />
          {sessionInfo?.model && <span className="composer-model">{sessionInfo.model}</span>}
          <button
            type="button"
            className="btn btn-icon"
            title="New session"
            onClick={onNewSession}
          >
            ⊕
          </button>
        </div>
      </div>
    </div>
  );
}
