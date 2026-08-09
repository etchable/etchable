import { AssistantRuntimeProvider } from "@assistant-ui/react";
import {
  IconCheck,
  IconChevronDown,
  IconCrosshair,
  IconPlus,
  IconX,
} from "@etchable/ui";
import type { SessionInfo } from "../state";
import type { SessionSummary } from "../types";
import type { ChatItem } from "./messages";
import { useChatRuntime } from "./runtime";
import { Thread } from "./assistant-ui/thread";
import { TooltipIconButton } from "./assistant-ui/tooltip-icon-button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "./ui/dropdown-menu";

const SUGGESTIONS = [
  "What does this board do?",
  "Check for unconnected nets",
  "Walk me through the power path",
];

type ChatProps = {
  transcript: ChatItem[];
  agentRunning: boolean;
  selection: string[];
  sessionInfo: SessionInfo | null;
  /** Resumable sessions for this workspace (newest first). */
  sessions: SessionSummary[];
  onSend: (text: string) => void;
  onRespondPermission: (requestId: string, allow: boolean) => void;
  onInterrupt: () => void;
  onNewSession: () => void;
  onClearSelection: () => void;
  onResumeSession: (sessionId: string) => void;
};

/** Canvas selection riding along as context — shown inside the composer. */
function SelectionChip({
  selection,
  onClear,
}: {
  selection: string[];
  onClear: () => void;
}) {
  if (selection.length === 0) return null;
  const joined = selection.join(", ");
  const preview = joined.length > 48 ? joined.slice(0, 48) + "…" : joined;
  return (
    <div className="flex min-w-0 items-center gap-1.5 rounded-full border border-sky/40 bg-sky/10 px-2.5 py-1 text-xxs">
      <span className="flex flex-none text-sky">
        <IconCrosshair size={12} />
      </span>
      <span className="min-w-0 flex-1 truncate">
        {selection.length} selected — included as context
        <span className="font-mono text-[10px] text-ink/55"> · {preview}</span>
      </span>
      <button
        type="button"
        className="flex flex-none cursor-pointer text-ink/55 hover:text-ink/92"
        title="Clear selection"
        onClick={onClear}
      >
        <IconX size={11} />
      </button>
    </div>
  );
}

/** The conversation's display name: its opening question. The store keeps
    the same fact per session (SessionSummary.title, set once at init), so
    live derivation and the picker's list agree. */
function deriveTitle(transcript: ChatItem[]): string | null {
  const first = transcript.find((i) => i.kind === "user");
  const text = first?.text.replace(/\s+/g, " ").trim();
  return text || null;
}

/** Header title-as-menu: current chat name, click to switch chats or start
    a new one. Resume kills any live agent backend-side, so switching is
    always safe. */
function SessionMenu({
  title,
  sessions,
  currentSessionId,
  onNewSession,
  onResumeSession,
}: {
  title: string | null;
  sessions: SessionSummary[];
  currentSessionId: string | null;
  onNewSession: () => void;
  onResumeSession: (sessionId: string) => void;
}) {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger className="group flex max-w-[75%] cursor-pointer items-center gap-1 rounded-md px-2 py-1 outline-none hover:bg-ink/5 data-[state=open]:bg-ink/5">
        <span className="truncate">{title ?? "New chat"}</span>
        <IconChevronDown
          size={11}
          className="flex-none text-ink/40 transition-transform group-data-[state=open]:rotate-180"
        />
      </DropdownMenuTrigger>
      <DropdownMenuContent align="center">
        <DropdownMenuItem onSelect={onNewSession}>
          <IconPlus size={12} className="flex-none text-ink/55" />
          New chat
        </DropdownMenuItem>
        <DropdownMenuSeparator />
        {sessions.length === 0 ? (
          <div className="px-2 py-1.5 text-xxs text-ink/40">
            No previous chats
          </div>
        ) : (
          sessions.map((s) => (
            <DropdownMenuItem
              key={s.sessionId}
              onSelect={() => {
                if (s.sessionId !== currentSessionId) onResumeSession(s.sessionId);
              }}
            >
              <span className="min-w-0 flex-1 truncate">
                {s.title ?? "Untitled chat"}
              </span>
              {s.sessionId === currentSessionId && (
                <IconCheck size={12} className="flex-none text-copper" />
              )}
            </DropdownMenuItem>
          ))
        )}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

function SessionControls({
  sessionInfo,
  onNewSession,
}: {
  sessionInfo: SessionInfo | null;
  onNewSession: () => void;
}) {
  return (
    <>
      <TooltipIconButton tooltip="New session" onClick={onNewSession}>
        <IconPlus size={13} />
      </TooltipIconButton>
      {sessionInfo?.model && (
        <span className="truncate font-mono text-[9.5px] text-ink/35">
          {sessionInfo.model}
        </span>
      )}
    </>
  );
}

export default function Chat(props: ChatProps) {
  const runtime = useChatRuntime({
    transcript: props.transcript,
    agentRunning: props.agentRunning,
    onSend: props.onSend,
    onRespondPermission: props.onRespondPermission,
    onInterrupt: props.onInterrupt,
  });

  // Offer to pick up where the user left off: no live session, empty
  // transcript, and the store remembers one for this workspace.
  const resumable =
    !props.sessionInfo &&
    !props.agentRunning &&
    props.transcript.length === 0 &&
    props.sessions.length > 0
      ? props.sessions[0]
      : null;

  return (
    <AssistantRuntimeProvider runtime={runtime}>
      <Thread
        suggestions={SUGGESTIONS}
        header={
          <SessionMenu
            title={deriveTitle(props.transcript)}
            sessions={props.sessions}
            currentSessionId={props.sessionInfo?.sessionId ?? null}
            onNewSession={props.onNewSession}
            onResumeSession={props.onResumeSession}
          />
        }
        // Before the init event there is no session yet — the spawn is the
        // slow part worth naming. After that, silent gaps are the model
        // working (for current models: redacted thinking).
        workingLabel={props.sessionInfo ? "Thinking…" : "Starting Claude…"}
        welcomeExtra={
          resumable && (
            <button
              type="button"
              className="max-w-full cursor-pointer truncate rounded-full border border-sky/40 bg-sky/10 px-3 py-1 text-xxs text-ink/70 transition-colors hover:bg-sky/20"
              title={resumable.title ?? resumable.sessionId}
              onClick={() => props.onResumeSession(resumable.sessionId)}
            >
              Resume last session{resumable.title ? ` — ${resumable.title}` : ""}
            </button>
          )
        }
        composerAccessory={
          <SelectionChip selection={props.selection} onClear={props.onClearSelection} />
        }
        composerControls={
          <SessionControls sessionInfo={props.sessionInfo} onNewSession={props.onNewSession} />
        }
      />
    </AssistantRuntimeProvider>
  );
}
