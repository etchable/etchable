import { AssistantRuntimeProvider } from "@assistant-ui/react";
import { IconCrosshair, IconPlus, IconX } from "@etchable/ui";
import type { SessionInfo } from "../state";
import type { SessionSummary } from "../types";
import type { ChatItem } from "./messages";
import { useChatRuntime } from "./runtime";
import { Thread } from "./assistant-ui/thread";
import { TooltipIconButton } from "./assistant-ui/tooltip-icon-button";

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
