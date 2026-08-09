// Vendored from the assistant-ui shadcn registry (tool-fallback), adapted:
// local ui imports, Phosphor icons, sidebar (11px) register, and etchable
// tool-name treatment — `mcp__etchable__*` tools render bare with a "canvas"
// pill, and the trigger carries a one-line args preview (file path, command…)
// like the rest of the app's instrument chrome.

import { memo, useCallback, useRef, useState } from "react";
import { CaretDown, Check, CircleNotch, WarningCircle, XCircle } from "@phosphor-icons/react";
import {
  useScrollLock,
  useToolCallElapsed,
  type ToolCallMessagePartProps,
  type ToolCallMessagePartStatus,
  type ToolCallMessagePartComponent,
} from "@assistant-ui/react";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "../ui/collapsible";
import { cn } from "../ui/utils";
import { Button } from "../ui/button";
import { MCP_PREFIX, previewInput } from "../messages";

const ANIMATION_DURATION = 200;

const pressable = "active:scale-[0.98]";

export type ToolFallbackRootProps = Omit<
  React.ComponentProps<typeof Collapsible>,
  "open" | "onOpenChange"
> & {
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
  defaultOpen?: boolean;
};

function ToolFallbackRoot({
  className,
  open: controlledOpen,
  onOpenChange: controlledOnOpenChange,
  defaultOpen = false,
  children,
  ...props
}: ToolFallbackRootProps) {
  const collapsibleRef = useRef<HTMLDivElement>(null);
  const [uncontrolledOpen, setUncontrolledOpen] = useState(defaultOpen);
  const lockScroll = useScrollLock(collapsibleRef, ANIMATION_DURATION);

  const isControlled = controlledOpen !== undefined;
  const isOpen = isControlled ? controlledOpen : uncontrolledOpen;

  const handleOpenChange = useCallback(
    (open: boolean) => {
      lockScroll();
      if (!isControlled) {
        setUncontrolledOpen(open);
      }
      controlledOnOpenChange?.(open);
    },
    [lockScroll, isControlled, controlledOnOpenChange],
  );

  return (
    <Collapsible
      ref={collapsibleRef}
      data-slot="tool-fallback-root"
      open={isOpen}
      onOpenChange={handleOpenChange}
      className={cn("aui-tool-fallback-root group/tool-fallback-root w-full", className)}
      style={
        {
          "--animation-duration": `${ANIMATION_DURATION}ms`,
        } as React.CSSProperties
      }
      {...props}
    >
      {children}
    </Collapsible>
  );
}

type ToolStatus = ToolCallMessagePartStatus["type"];

const statusIconMap: Record<ToolStatus, React.ElementType> = {
  running: CircleNotch,
  complete: Check,
  incomplete: XCircle,
  "requires-action": WarningCircle,
};

const formatToolDuration = (ms: number) => {
  if (ms < 1000) return "<1s";
  const seconds = ms / 1000;
  if (seconds < 10) return `${(Math.floor(seconds * 10) / 10).toFixed(1)}s`;
  if (seconds < 60) return `${Math.floor(seconds)}s`;
  return `${Math.floor(seconds / 60)}m ${Math.floor(seconds % 60)}s`;
};

function ToolFallbackDuration({ className, ...props }: React.ComponentProps<"span">) {
  const elapsedMs = useToolCallElapsed();
  if (elapsedMs === undefined) return null;

  return (
    <span
      data-slot="tool-fallback-duration"
      className={cn("aui-tool-fallback-duration text-muted-foreground text-[10px] tabular-nums", className)}
      {...props}
    >
      {formatToolDuration(elapsedMs)}
    </span>
  );
}

function ToolFallbackTrigger({
  toolName,
  args,
  status,
  className,
  ...props
}: React.ComponentProps<typeof CollapsibleTrigger> & {
  toolName: string;
  args?: unknown;
  status?: ToolCallMessagePartStatus;
}) {
  const statusType = status?.type ?? "complete";
  const isRunning = statusType === "running";
  const isCancelled = status?.type === "incomplete" && status.reason === "cancelled";

  const isMcp = toolName.startsWith(MCP_PREFIX);
  const displayName = isMcp ? toolName.slice(MCP_PREFIX.length) : toolName;
  const preview = args === undefined ? "" : previewInput(args);

  const Icon = statusIconMap[statusType];

  return (
    <CollapsibleTrigger
      data-slot="tool-fallback-trigger"
      className={cn(
        "aui-tool-fallback-trigger group/trigger text-muted-foreground hover:text-foreground flex w-full min-w-0 origin-left cursor-pointer items-center gap-1.5 py-1 text-xxs transition-[color,scale] active:scale-[0.98]",
        className,
      )}
      {...props}
    >
      <Icon
        data-slot="tool-fallback-trigger-icon"
        className={cn(
          "aui-tool-fallback-trigger-icon size-3.5 shrink-0",
          isCancelled && "text-muted-foreground",
          isRunning && "animate-spin [animation-duration:0.6s]",
        )}
      />
      <span
        data-slot="tool-fallback-trigger-label"
        className={cn(
          "aui-tool-fallback-trigger-label-wrapper relative inline-block shrink-0 text-start font-mono leading-none font-semibold",
          isCancelled && "text-muted-foreground line-through",
        )}
      >
        <span>{displayName}</span>
        {isRunning && (
          <span
            aria-hidden
            data-slot="tool-fallback-trigger-shimmer"
            className="aui-tool-fallback-trigger-shimmer shimmer pointer-events-none absolute inset-0 motion-reduce:animate-none"
          >
            {displayName}
          </span>
        )}
      </span>
      {isMcp && (
        <span className="shrink-0 rounded-full border border-sky/40 px-[5px] font-mono text-[9px] leading-[14px] text-sky">
          canvas
        </span>
      )}
      <span className="min-w-0 flex-1 truncate text-start font-mono text-[10px] text-muted-foreground/70">
        {preview}
      </span>
      <ToolFallbackDuration />
      <CaretDown
        data-slot="tool-fallback-trigger-chevron"
        className={cn(
          "aui-tool-fallback-trigger-chevron size-3 shrink-0",
          "transition-transform duration-(--animation-duration) ease-[cubic-bezier(0.32,0.72,0,1)] motion-reduce:transition-none",
          "-rotate-90",
          "group-data-open/trigger:rotate-0",
          "group-data-panel-open/trigger:rotate-0",
        )}
      />
    </CollapsibleTrigger>
  );
}

function ToolFallbackContent({
  className,
  children,
  ...props
}: React.ComponentProps<typeof CollapsibleContent>) {
  return (
    <CollapsibleContent
      data-slot="tool-fallback-content"
      className={cn(
        "aui-tool-fallback-content relative overflow-hidden text-xxs outline-none",
        "group/collapsible-content ease-[cubic-bezier(0.32,0.72,0,1)] motion-reduce:animate-none",
        "data-closed:animate-collapsible-up",
        "data-open:animate-collapsible-down",
        "data-closed:fill-mode-forwards",
        "data-closed:pointer-events-none",
        "data-open:duration-(--animation-duration)",
        "data-closed:duration-(--animation-duration)",
        className,
      )}
      {...props}
    >
      <div
        className={cn(
          "flex flex-col gap-2 ps-5 pt-1 pb-2 ease-[cubic-bezier(0.32,0.72,0,1)] motion-reduce:animate-none",
          "group-data-open/collapsible-content:animate-in group-data-open/collapsible-content:fade-in-0 group-data-open/collapsible-content:slide-in-from-top-1",
          "group-data-closed/collapsible-content:animate-out group-data-closed/collapsible-content:fade-out-0 group-data-closed/collapsible-content:slide-out-to-top-1",
          "group-data-closed/collapsible-content:duration-(--animation-duration) group-data-open/collapsible-content:duration-(--animation-duration)",
        )}
      >
        {children}
      </div>
    </CollapsibleContent>
  );
}

const CODE_BLOCK =
  "max-h-[200px] select-text overflow-auto whitespace-pre-wrap wrap-anywhere rounded-md bg-white p-2 font-mono text-[10px] shadow-[inset_0_0_0_0.5px_rgba(35,43,63,0.08)]";

function ToolFallbackArgs({
  argsText,
  className,
  ...props
}: React.ComponentProps<"div"> & {
  argsText?: string;
}) {
  if (!argsText) return null;

  return (
    <div data-slot="tool-fallback-args" className={cn("aui-tool-fallback-args", className)} {...props}>
      <pre className={cn("aui-tool-fallback-args-value text-muted-foreground", CODE_BLOCK)}>
        {argsText}
      </pre>
    </div>
  );
}

function ToolFallbackResult({
  result,
  isError,
  className,
  ...props
}: React.ComponentProps<"div"> & {
  result?: unknown;
  isError?: boolean;
}) {
  if (result === undefined) return null;

  return (
    <div data-slot="tool-fallback-result" className={cn("aui-tool-fallback-result", className)} {...props}>
      <pre
        className={cn(
          "aui-tool-fallback-result-content",
          CODE_BLOCK,
          isError ? "text-alert" : "text-foreground/90",
        )}
      >
        {(typeof result === "string" ? result : JSON.stringify(result, null, 2)) ||
          "(empty result)"}
      </pre>
    </div>
  );
}

function ToolFallbackError({
  status,
  className,
  ...props
}: React.ComponentProps<"div"> & {
  status?: ToolCallMessagePartStatus;
}) {
  if (status?.type !== "incomplete") return null;

  const error = status.error;
  const errorText = error ? (typeof error === "string" ? error : JSON.stringify(error)) : null;

  if (!errorText) return null;

  const isCancelled = status.reason === "cancelled";
  const headerText = isCancelled ? "Cancelled:" : "Error:";

  return (
    <div data-slot="tool-fallback-error" className={cn("aui-tool-fallback-error", className)} {...props}>
      <p className="aui-tool-fallback-error-header text-alert font-semibold">{headerText}</p>
      <p className="aui-tool-fallback-error-reason text-muted-foreground">{errorText}</p>
    </div>
  );
}

const APPROVED_RESULT = "Approved by user";
const DENIED_RESULT = "User denied tool execution";

function ToolFallbackApproval({
  className,
  addResult,
  resume,
  interrupt,
  approval,
  respondToApproval,
  ...props
}: React.ComponentProps<"div"> &
  Partial<Pick<ToolCallMessagePartProps, "addResult" | "resume" | "respondToApproval">> & {
    interrupt?: ToolCallMessagePartProps["interrupt"];
    approval?: ToolCallMessagePartProps["approval"];
  }) {
  const [submitted, setSubmitted] = useState(false);

  if (approval != null && (approval.approved !== undefined || approval.resolution !== undefined))
    return null;

  const respond = (approved: boolean) => {
    if (submitted) return;
    if (approval != null && approval.approved === undefined && respondToApproval) {
      respondToApproval({ approved });
    } else if (interrupt) {
      resume?.({ approved });
    } else {
      addResult?.(approved ? APPROVED_RESULT : DENIED_RESULT);
    }
    setSubmitted(true);
  };

  return (
    <div
      data-slot="tool-fallback-approval"
      className={cn("aui-tool-fallback-approval flex items-center gap-2 pt-1", className)}
      {...props}
    >
      <Button size="sm" className={pressable} onClick={() => respond(true)} disabled={submitted}>
        Allow
      </Button>
      <Button
        size="sm"
        variant="outline"
        className={pressable}
        onClick={() => respond(false)}
        disabled={submitted}
      >
        Deny
      </Button>
      <span className="animate-pulse text-[10px] text-muted-foreground">waiting for you</span>
    </div>
  );
}

const ToolFallbackImpl: ToolCallMessagePartComponent = ({
  toolName,
  args,
  argsText,
  result,
  isError,
  status,
  addResult,
  resume,
  interrupt,
  approval,
  respondToApproval,
}) => {
  const isCancelled = status?.type === "incomplete" && status.reason === "cancelled";
  // External-store approval parts never get a requires-action status from
  // the runtime — a pending `approval` IS the requires-action signal here.
  const needsApproval =
    status?.type === "requires-action" ||
    (approval != null && approval.approved === undefined && approval.resolution === undefined);
  const displayStatus: ToolCallMessagePartStatus | undefined = needsApproval
    ? { type: "requires-action", reason: "interrupt" }
    : status;

  const [open, setOpen] = useState(needsApproval);
  const [prevNeedsApproval, setPrevNeedsApproval] = useState(needsApproval);
  if (needsApproval !== prevNeedsApproval) {
    setPrevNeedsApproval(needsApproval);
    if (needsApproval) setOpen(true);
  }

  return (
    <ToolFallbackRoot open={open} onOpenChange={setOpen}>
      <ToolFallbackTrigger toolName={toolName} args={args} status={displayStatus} />
      <ToolFallbackContent>
        <ToolFallbackError status={status} />
        <ToolFallbackArgs argsText={argsText} className={cn(isCancelled && "opacity-60")} />
        {needsApproval && (
          <ToolFallbackApproval
            addResult={addResult}
            resume={resume}
            interrupt={interrupt}
            approval={approval}
            respondToApproval={respondToApproval}
          />
        )}
        {!isCancelled && <ToolFallbackResult result={result} isError={isError} />}
      </ToolFallbackContent>
    </ToolFallbackRoot>
  );
};

const ToolFallback = memo(ToolFallbackImpl) as unknown as ToolCallMessagePartComponent & {
  Root: typeof ToolFallbackRoot;
  Trigger: typeof ToolFallbackTrigger;
  Content: typeof ToolFallbackContent;
  Args: typeof ToolFallbackArgs;
  Result: typeof ToolFallbackResult;
  Error: typeof ToolFallbackError;
  Approval: typeof ToolFallbackApproval;
};

ToolFallback.displayName = "ToolFallback";
ToolFallback.Root = ToolFallbackRoot;
ToolFallback.Trigger = ToolFallbackTrigger;
ToolFallback.Content = ToolFallbackContent;
ToolFallback.Args = ToolFallbackArgs;
ToolFallback.Result = ToolFallbackResult;
ToolFallback.Error = ToolFallbackError;
ToolFallback.Approval = ToolFallbackApproval;

export {
  ToolFallback,
  ToolFallbackRoot,
  ToolFallbackTrigger,
  ToolFallbackContent,
  ToolFallbackArgs,
  ToolFallbackResult,
  ToolFallbackError,
  ToolFallbackApproval,
};
