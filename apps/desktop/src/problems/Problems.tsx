import { useMemo, useState } from "react";
import { IconChevronDown, IconChevronRight, IconWarning, IconX } from "@etchable/ui";
import type { Diag } from "../types";

type ProblemsProps = {
  diagnostics: Diag[];
  /// Project-level issues (manifest/card parse problems) — plain strings,
  /// not build diagnostics.
  projectProblems?: string[];
  onSelectDiag: (diag: Diag) => void;
};

function sevIcon(sev: Diag["severity"]) {
  switch (sev) {
    case "error":
      return (
        <span className="text-alert">
          <IconX size={12} />
        </span>
      );
    case "warning":
      return (
        <span className="text-warn-deep">
          <IconWarning size={12} />
        </span>
      );
    case "advice":
      return (
        <span className="text-ink/35">
          <IconChevronRight size={12} />
        </span>
      );
  }
}

function DiagRow({ diag, onClick }: { diag: Diag; onClick: () => void }) {
  const loc = diag.file ? `${diag.file}${diag.line !== undefined ? ":" + diag.line : ""}` : "";
  return (
    <button
      type="button"
      className="flex w-full cursor-pointer items-start gap-[9px] rounded-md px-2 py-[5px] text-left text-[11.5px] transition-colors hover:bg-ink/4"
      onClick={onClick}
    >
      <span className="flex w-3.5 flex-none justify-center pt-0.5">
        {sevIcon(diag.severity)}
      </span>
      <span className="flex min-w-0 flex-col gap-px">
        <span className="select-text wrap-anywhere">{diag.message}</span>
        {loc && <span className="font-mono text-[10px] text-ink/35">{loc}</span>}
      </span>
    </button>
  );
}

export default function Problems({ diagnostics, projectProblems = [], onSelectDiag }: ProblemsProps) {
  const [showAdvice, setShowAdvice] = useState(false);

  const groups = useMemo(() => {
    const errors: Diag[] = [];
    const warnings: Diag[] = [];
    const advice: Diag[] = [];
    for (const d of diagnostics) {
      if (d.suppressed) continue;
      if (d.severity === "error") errors.push(d);
      else if (d.severity === "warning") warnings.push(d);
      else advice.push(d);
    }
    return { errors, warnings, advice };
  }, [diagnostics]);

  const empty =
    groups.errors.length === 0 &&
    groups.warnings.length === 0 &&
    groups.advice.length === 0 &&
    projectProblems.length === 0;

  const groupTitle = "px-0.5 pb-1 font-mono text-[10px] uppercase tracking-wider";

  return (
    <div className="scroll-minimal flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto p-2.5">
      {empty && <div className="m-auto text-xs text-ink/35">No problems — clean build.</div>}

      {projectProblems.length > 0 && (
        <div className="flex flex-col gap-0.5">
          <div className={`${groupTitle} text-warn-deep`}>
            {projectProblems.length} project problem{projectProblems.length === 1 ? "" : "s"}
          </div>
          {projectProblems.map((p, i) => (
            <div
              key={`p${i}`}
              className="flex items-start gap-[9px] rounded-md px-2 py-[5px] text-[11.5px]"
            >
              <span className="flex w-3.5 flex-none justify-center pt-0.5 text-warn-deep">
                <IconWarning size={12} />
              </span>
              <span className="select-text wrap-anywhere">{p}</span>
            </div>
          ))}
        </div>
      )}

      {groups.errors.length > 0 && (
        <div className="flex flex-col gap-0.5">
          <div className={`${groupTitle} text-alert`}>
            {groups.errors.length} error{groups.errors.length === 1 ? "" : "s"}
          </div>
          {groups.errors.map((d, i) => (
            <DiagRow key={`e${i}`} diag={d} onClick={() => onSelectDiag(d)} />
          ))}
        </div>
      )}

      {groups.warnings.length > 0 && (
        <div className="flex flex-col gap-0.5">
          <div className={`${groupTitle} text-warn-deep`}>
            {groups.warnings.length} warning{groups.warnings.length === 1 ? "" : "s"}
          </div>
          {groups.warnings.map((d, i) => (
            <DiagRow key={`w${i}`} diag={d} onClick={() => onSelectDiag(d)} />
          ))}
        </div>
      )}

      {groups.advice.length > 0 && (
        <div className="flex flex-col gap-0.5">
          <button
            type="button"
            className="inline-flex cursor-pointer items-center gap-1 px-0.5 py-[3px] text-left font-mono text-[10.5px] text-ink/35 hover:text-ink/55"
            onClick={() => setShowAdvice((v) => !v)}
          >
            {showAdvice ? <IconChevronDown size={11} /> : <IconChevronRight size={11} />}{" "}
            {showAdvice ? "hide" : "show"} {groups.advice.length} advice
          </button>
          {showAdvice &&
            groups.advice.map((d, i) => (
              <DiagRow key={`a${i}`} diag={d} onClick={() => onSelectDiag(d)} />
            ))}
        </div>
      )}
    </div>
  );
}
