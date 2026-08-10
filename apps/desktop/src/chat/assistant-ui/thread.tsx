// Vendored from the assistant-ui shadcn registry (thread), adapted for
// etchable: no attachments/dictation/branching/edit (the embedded-agent
// runtime supports none of them), Phosphor icons, system-message rows, a
// per-turn cost/duration footer, and slots threaded in from Chat.tsx: a
// panel header (session title / chat picker) plus two composer slots
// (selection-context chip + session controls).

import { MarkdownText } from "./markdown-text";
import {
  Reasoning,
  ReasoningContent,
  ReasoningRoot,
  ReasoningText,
  ReasoningTrigger,
} from "./reasoning";
import { ShimmerText } from "./shimmer-text";
import { ToolFallback } from "./tool-fallback";
import { TooltipIconButton } from "./tooltip-icon-button";
import {
  AnsweredApprovalRow,
  ApprovalRow,
  PlanRow,
  PulseDots,
  RunAwareWorkRow,
  WorkingTimer,
} from "./work-log";
import { cn } from "../ui/utils";
import {
  ActionBarPrimitive,
  AuiIf,
  type AssistantState,
  ComposerPrimitive,
  ErrorPrimitive,
  groupPartByType,
  MessagePrimitive,
  ThreadPrimitive,
  useAuiState,
} from "@assistant-ui/react";
import { ArrowDown, ArrowUp, Check, Copy, Stop } from "@phosphor-icons/react";
import {
  createContext,
  useContext,
  useEffect,
  useRef,
  useState,
  type FC,
  type ReactNode,
} from "react";
import { activityLabel, formatResultFooter } from "../messages";
import { isPlanTool, turnResultOf, turnStartedAtOf } from "../runtime";

export type ThreadProps = {
  /** Rendered centered in the panel header (session title / chat picker). */
  header?: ReactNode;
  /** Rendered inside the composer shell, above the input (selection chip). */
  composerAccessory?: ReactNode;
  /** Rendered in the composer's action row, start side (model, new session). */
  composerControls?: ReactNode;
  /** Starter prompts shown on the empty thread. */
  suggestions?: string[];
  /** Extra content under the welcome (e.g. resume-last-session chip). */
  welcomeExtra?: ReactNode;
  /** What the silent-work shimmer says while a turn has no output yet. */
  workingLabel?: string;
};

const ThreadSlotsContext = createContext<ThreadProps>({});

const isNewChatView = (s: AssistantState) => s.thread.messages.length === 0;

export const Thread: FC<ThreadProps> = (slots) => {
  return (
    <ThreadSlotsContext.Provider value={slots}>
      <ThreadRoot />
    </ThreadSlotsContext.Provider>
  );
};

const ThreadRoot: FC = () => {
  const { header } = useContext(ThreadSlotsContext);
  return (
    <ThreadPrimitive.Root
      className="aui-root aui-thread-root bg-background @container flex h-full flex-col"
      style={{
        ["--composer-radius" as string]: "20px",
      }}
    >
      {/* Panel header on the same surface as the thread, so the chat reads
          as one sheet; the fade strip under it dissolves scrolling content
          into the background instead of clipping it. Placeholder title until
          the header grows real controls. */}
      <div className="relative z-10 flex-none bg-background">
        {/* h-11 = the Shell titlebar's 44px (theme.css .shell-titlebar), so
            the panel header lines up with the window chrome. */}
        <div className="flex h-11 items-center justify-center text-xs font-medium text-ink/65">
          {header ?? "Chat"}
        </div>
        <div className="pointer-events-none absolute inset-x-0 top-full h-3 bg-[linear-gradient(to_bottom,var(--color-background),transparent)]" />
      </div>
      {/* overflow-x-HIDDEN on purpose: the panel must never scroll
          horizontally — wide content (code blocks, tool output) scrolls
          inside its own container. An auto x-scrollbar here makes the whole
          thread jitter whenever something transiently overflows. */}
      {/* turnAnchor "bottom" = classic stick-to-bottom. The template's
          "top" anchoring (pin the user message to the top, blank space
          below for the streaming answer) reads as "it scrolled to the
          top" against this panel's work-log turns. */}
      <ThreadPrimitive.Viewport
        turnAnchor="bottom"
        data-slot="aui_thread-viewport"
        className="scroll-minimal relative flex flex-1 flex-col overflow-x-hidden overflow-y-scroll scroll-smooth"
      >
        {/* Asymmetric padding on purpose: the always-on scroll-minimal
            scrollbar (6px, classic mode — styling ::-webkit-scrollbar opts
            out of overlay bars) occupies the right edge, so pr-1 + 6px
            matches pl-2.5 visually. */}
        <div className="mx-auto flex w-full flex-1 flex-col pl-2.5 pr-1 pt-3">
          {/* Empty thread: welcome + starter chips centered in the space
              above the bottom-docked composer. */}
          <AuiIf condition={isNewChatView}>
            <div className="flex flex-1 flex-col items-center justify-center gap-5">
              <ThreadWelcome />
              <AuiIf condition={(s) => s.composer.isEmpty}>
                <ThreadSuggestions />
              </AuiIf>
              <WelcomeExtra />
            </div>
          </AuiIf>

          <div
            data-slot="aui_message-group"
            className="mb-6 flex flex-col gap-y-4 empty:hidden"
          >
            <ThreadPrimitive.Messages components={{ Message: ThreadMessage }} />
          </div>

          <ThreadPrimitive.ViewportFooter className="aui-thread-viewport-footer bg-background sticky bottom-0 mt-auto flex flex-col gap-2 overflow-visible rounded-t-(--composer-radius) pb-2.5">
            <ThreadScrollToBottom />
            <Composer />
          </ThreadPrimitive.ViewportFooter>
        </div>
      </ThreadPrimitive.Viewport>
    </ThreadPrimitive.Root>
  );
};

const ThreadMessage: FC = () => {
  const role = useAuiState((s) => s.message.role);

  if (role === "user") return <UserMessage />;
  if (role === "system") return <SystemMessage />;
  return <AssistantMessage />;
};

const ThreadScrollToBottom: FC = () => {
  return (
    <ThreadPrimitive.ScrollToBottom asChild>
      <TooltipIconButton
        tooltip="Scroll to bottom"
        variant="outline"
        className="aui-thread-scroll-to-bottom bg-background absolute -top-10 z-10 size-8 self-center rounded-full shadow-seg disabled:invisible"
      >
        <ArrowDown className="size-4" />
      </TooltipIconButton>
    </ThreadPrimitive.ScrollToBottom>
  );
};

const ThreadWelcome: FC = () => {
  return (
    <div className="aui-thread-welcome-root flex flex-col items-center px-4 text-center">
      <h1 className="aui-thread-welcome-message font-display text-lg font-extrabold tracking-tight">
        Ask about the board
      </h1>
      <p className="mt-1 text-xxs text-muted-foreground">
        Claude can read the schematic and edit the source.
      </p>
    </div>
  );
};

const WelcomeExtra: FC = () => {
  const { welcomeExtra } = useContext(ThreadSlotsContext);
  if (!welcomeExtra) return null;
  return <>{welcomeExtra}</>;
};

const ThreadSuggestions: FC = () => {
  const { suggestions = [] } = useContext(ThreadSlotsContext);
  if (suggestions.length === 0) return null;
  return (
    <div className="aui-thread-welcome-suggestions flex w-full flex-wrap items-center justify-center gap-1.5 px-2">
      {suggestions.map((prompt) => (
        <ThreadPrimitive.Suggestion
          key={prompt}
          prompt={prompt}
          method="replace"
          autoSend
          className="cursor-pointer rounded-full border border-border/60 px-3 py-1 text-xxs text-foreground/80 whitespace-nowrap transition-colors hover:bg-accent"
        >
          {prompt}
        </ThreadPrimitive.Suggestion>
      ))}
    </div>
  );
};

const Composer: FC = () => {
  const { composerAccessory, composerControls } = useContext(ThreadSlotsContext);
  return (
    <ComposerPrimitive.Root className="aui-composer-root relative flex w-full flex-col">
      {/* Focus ring rides the whole card, not the textarea: the theme's
          global sky :focus-visible outline is suppressed on the input and
          replaced by a copper ring here. */}
      <div
        data-slot="aui_composer-shell"
        className="flex w-full flex-col gap-1.5 rounded-(--composer-radius) bg-white p-2 shadow-island transition-[box-shadow] focus-within:shadow-island-lg focus-within:ring-2 focus-within:ring-copper"
      >
        {composerAccessory}
        <ComposerPrimitive.Input
          placeholder="Message Claude…"
          className="aui-composer-input caret-copper placeholder:text-muted-foreground/80 max-h-32 min-h-9 w-full select-text resize-none bg-transparent px-1.5 py-1 text-xs leading-relaxed outline-none"
          rows={1}
          autoFocus
          enterKeyHint="send"
          aria-label="Message input"
        />
        <div className="aui-composer-action-wrapper relative flex items-center justify-between">
          <div className="flex min-w-0 items-center gap-1.5">{composerControls}</div>
          <div className="flex flex-none items-center gap-1.5">
            <AuiIf condition={(s) => !s.thread.isRunning}>
              <ComposerPrimitive.Send asChild>
                <TooltipIconButton
                  tooltip="Send message"
                  side="top"
                  type="button"
                  variant="default"
                  size="icon"
                  className="aui-composer-send size-7 rounded-full"
                  aria-label="Send message"
                >
                  <ArrowUp weight="bold" className="size-4" />
                </TooltipIconButton>
              </ComposerPrimitive.Send>
            </AuiIf>
            <AuiIf condition={(s) => s.thread.isRunning}>
              <ComposerPrimitive.Cancel asChild>
                <TooltipIconButton
                  tooltip="Stop"
                  side="top"
                  type="button"
                  variant="default"
                  size="icon"
                  className="aui-composer-cancel size-7 rounded-full"
                  aria-label="Stop generating"
                >
                  <Stop weight="fill" className="size-3.5 animate-pulse" />
                </TooltipIconButton>
              </ComposerPrimitive.Cancel>
            </AuiIf>
          </div>
        </div>
      </div>
    </ComposerPrimitive.Root>
  );
};

const MessageError: FC = () => {
  return (
    <MessagePrimitive.Error>
      <ErrorPrimitive.Root className="aui-message-error-root border-alert/40 bg-alert/5 text-alert mt-2 rounded-md border p-2.5 text-xxs">
        <ErrorPrimitive.Message className="aui-message-error-message line-clamp-2" />
      </ErrorPrimitive.Root>
    </MessagePrimitive.Error>
  );
};

const AssistantMessage: FC = () => {
  return (
    <MessagePrimitive.Root
      data-slot="aui_assistant-message-root"
      data-role="assistant"
      className="fade-in slide-in-from-bottom-1 animate-in relative min-w-0 max-w-full duration-150"
    >
      <div
        data-slot="aui_assistant-message-content"
        className="text-foreground min-w-0 select-text text-xs leading-relaxed wrap-break-word"
      >
        <PlanRow />
        <MessagePrimitive.GroupedParts
          groupBy={groupPartByType({
            reasoning: ["group-reasoning"],
            "tool-call": ["group-tool"],
            "standalone-tool-call": [],
          })}
        >
          {({ part, children }) => {
            switch (part.type) {
              case "group-tool":
                // Work-log: a quiet stack of compact rows (t3code-style).
                return (
                  <div data-slot="aui_work-log" className="flex min-w-0 flex-col gap-px py-0.5">
                    {children}
                  </div>
                );
              case "group-reasoning": {
                const running = part.status.type === "running";
                return (
                  <ReasoningRoot streaming={running}>
                    <ReasoningTrigger active={running} />
                    <ReasoningContent aria-busy={running}>
                      <ReasoningText>{children}</ReasoningText>
                    </ReasoningContent>
                  </ReasoningRoot>
                );
              }
              case "text":
                return <MarkdownText />;
              case "reasoning":
                return <Reasoning {...part} />;
              case "tool-call":
                if (part.toolUI) return part.toolUI;
                // A gated call arrives as TWO parts (the tool_use + the
                // permission). Pending: the ApprovalRow is the single UI
                // and the matching work row hides. Answered: the executed
                // (or failed) work row is the record; the approval part
                // only survives as a denied line when no call matches.
                if (part.approval) {
                  const answered =
                    part.approval.approved !== undefined ||
                    part.approval.resolution !== undefined;
                  return answered ? (
                    <AnsweredApprovalRow {...part} />
                  ) : (
                    <ApprovalRow {...part} />
                  );
                }
                if (part.status.type === "requires-action") {
                  return <ToolFallback {...part} />;
                }
                // Task/todo calls feed the plan bar, not rows.
                if (isPlanTool(part.toolName)) return null;
                return <RunAwareWorkRow {...part} />;
              case "indicator":
                return <WorkingIndicator />;
              default:
                return null;
            }
          }}
        </MessagePrimitive.GroupedParts>
        <MessageError />
      </div>

      {/* Fixed h-6 (the copy button's exact height) + outside margin, so the
          hover-revealed action bar can't grow the row. ms-auto (not
          justify-between) keeps the meta right-aligned when the bar is
          absent. */}
      <div data-slot="aui_assistant-message-footer" className="mt-1 flex h-6 items-center gap-2">
        <AssistantActionBar />
        <span className="ms-auto">
          <TurnMeta />
        </span>
      </div>
    </MessagePrimitive.Root>
  );
};

/** How long a tool's activity label lingers after the tool finishes. Fast
    canvas tools complete in milliseconds — without a hold the label would
    flash unreadably and the line would read "Thinking…" all turn. */
const ACTIVITY_HOLD_MS = 1200;

function useStickyActivity(value: string | null): string | null {
  const [held, setHeld] = useState<string | null>(null);
  useEffect(() => {
    if (value !== null) {
      setHeld(value);
      return;
    }
    if (held === null) return;
    const t = setTimeout(() => setHeld(null), ACTIVITY_HOLD_MS);
    return () => clearTimeout(t);
  }, [value, held]);
  return value ?? held;
}

/** The turn's single status line: the running tool's activity when there is
    one ("Reading the schematic…", briefly held so fast tools stay legible),
    otherwise the silent-work label. Hidden while text or visible reasoning
    is streaming — the content itself shows progress then. */
const WorkingIndicator: FC = () => {
  const { workingLabel = "Thinking…" } = useContext(ThreadSlotsContext);
  const pendingApproval = useAuiState((s) =>
    s.message.parts.some(
      (p) =>
        p.type === "tool-call" &&
        p.approval != null &&
        p.approval.approved === undefined &&
        p.approval.resolution === undefined,
    ),
  );
  const activity = useAuiState((s) => {
    for (const p of s.message.parts) {
      // Approval parts aren't executing — they're waiting on the user.
      if (p.type === "tool-call" && p.approval == null && p.status.type === "running") {
        return activityLabel(p.toolName, p.args);
      }
    }
    return null;
  });
  const streamingContent = useAuiState((s) => {
    const last = s.message.parts[s.message.parts.length - 1];
    return (
      last != null &&
      (last.type === "text" || last.type === "reasoning") &&
      last.status.type === "running"
    );
  });
  const startedAt = useAuiState((s) => turnStartedAtOf(s.message.metadata));
  // Synthesized pre-turn messages carry no metadata; fall back to mount time.
  const mountedAt = useRef(Date.now()).current;
  const shown = useStickyActivity(activity);
  // Blocked on the user: the approval card is the status.
  if (pendingApproval) return null;
  if (activity === null && streamingContent) return null;
  return (
    <div
      data-slot="aui_assistant-message-indicator"
      className="flex min-w-0 items-center gap-2 py-1 text-xxs text-muted-foreground"
      aria-label="Assistant is working"
    >
      <PulseDots />
      <span className="shrink-0">
        Working for <WorkingTimer startedAt={startedAt ?? mountedAt} />
      </span>
      <span className="min-w-0 truncate">
        <ShimmerText>· {shown ?? workingLabel}</ShimmerText>
      </span>
    </div>
  );
};

/** Turn footer: wall-clock time · duration · cost (t3code's "2:40:02 PM · 10s"). */
const TurnMeta: FC = () => {
  const meta = useAuiState((s) => turnResultOf(s.message.metadata));
  if (!meta) return null;
  const clock =
    meta.at !== undefined
      ? new Date(meta.at).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" })
      : null;
  const text = formatResultFooter(meta);
  return (
    <span className="font-mono text-[9.5px] text-muted-foreground/70">
      {meta.isError ? <span className="text-alert">{meta.subtype} · </span> : null}
      {clock ? `${clock} · ` : ""}
      {text}
    </span>
  );
};

const AssistantActionBar: FC = () => {
  return (
    <ActionBarPrimitive.Root
      hideWhenRunning
      autohide="not-last"
      className="aui-assistant-action-bar-root text-muted-foreground animate-in fade-in flex gap-1 duration-200"
    >
      <ActionBarPrimitive.Copy asChild>
        <TooltipIconButton tooltip="Copy">
          <AuiIf condition={(s) => s.message.isCopied}>
            <Check className="animate-in zoom-in-50 fade-in size-3.5 duration-200 ease-out" />
          </AuiIf>
          <AuiIf condition={(s) => !s.message.isCopied}>
            <Copy className="animate-in zoom-in-75 fade-in size-3.5 duration-150" />
          </AuiIf>
        </TooltipIconButton>
      </ActionBarPrimitive.Copy>
    </ActionBarPrimitive.Root>
  );
};

const UserMessage: FC = () => {
  return (
    <MessagePrimitive.Root
      data-slot="aui_user-message-root"
      className="fade-in slide-in-from-bottom-1 animate-in flex justify-end duration-150"
      data-role="user"
    >
      <div className="aui-user-message-content max-w-[85%] select-text rounded-xl rounded-br-[4px] border border-copper/25 bg-copper/8 px-2.5 py-[7px] text-xs whitespace-pre-wrap wrap-break-word">
        <MessagePrimitive.Parts />
      </div>
    </MessagePrimitive.Root>
  );
};

/** Backend/system notices: session errors, interrupts. Selectors return
    PRIMITIVES on purpose — a fresh object from a useAuiState selector is an
    unstable getSnapshot and loops React into "maximum update depth". */
const SystemMessage: FC = () => {
  // One selector per value, each returning a PRIMITIVE. `useAuiState` is a
  // useSyncExternalStore selector, so returning a fresh object literal makes
  // every snapshot compare unequal — React re-renders forever ("the result of
  // getSnapshot should be cached", then "Maximum update depth exceeded"). Any
  // system notice hit it: a session error, an interrupt, or resuming a session.
  const text = useAuiState((s) => {
    const part = s.message.content[0];
    return part?.type === "text" ? part.text : "";
  });
  const isError = useAuiState(
    (s) => (s.message.metadata?.custom as { isError?: boolean } | undefined)?.isError ?? false,
  );
  return (
    <MessagePrimitive.Root data-role="system">
      <div
        className={cn(
          "select-text rounded-r-md border-l-2 px-2.5 py-1.5 font-mono text-[10.5px] whitespace-pre-wrap wrap-anywhere",
          isError ? "border-alert bg-alert/5 text-alert" : "border-ink/8 bg-elev text-muted-foreground",
        )}
      >
        {text}
      </div>
    </MessagePrimitive.Root>
  );
};
