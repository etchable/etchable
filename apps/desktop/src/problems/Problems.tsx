import { useMemo, useState } from "react";
import type { Diag } from "../types";

type ProblemsProps = {
  diagnostics: Diag[];
  onSelectDiag: (diag: Diag) => void;
};

function sevIcon(sev: Diag["severity"]): string {
  switch (sev) {
    case "error":
      return "✗";
    case "warning":
      return "⚠";
    case "advice":
      return "ℹ";
  }
}

function DiagRow({ diag, onClick }: { diag: Diag; onClick: () => void }) {
  const loc = diag.file ? `${diag.file}${diag.line !== undefined ? ":" + diag.line : ""}` : "";
  return (
    <button type="button" className={`diag-row sev-${diag.severity}`} onClick={onClick}>
      <span className="diag-icon">{sevIcon(diag.severity)}</span>
      <span className="diag-main">
        <span className="diag-msg">{diag.message}</span>
        {loc && <span className="diag-loc">{loc}</span>}
      </span>
    </button>
  );
}

export default function Problems({ diagnostics, onSelectDiag }: ProblemsProps) {
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
    groups.errors.length === 0 && groups.warnings.length === 0 && groups.advice.length === 0;

  return (
    <div className="problems">
      {empty && <div className="problems-empty">No problems — clean build.</div>}

      {groups.errors.length > 0 && (
        <div className="diag-group">
          <div className="diag-group-title sev-error">
            {groups.errors.length} error{groups.errors.length === 1 ? "" : "s"}
          </div>
          {groups.errors.map((d, i) => (
            <DiagRow key={`e${i}`} diag={d} onClick={() => onSelectDiag(d)} />
          ))}
        </div>
      )}

      {groups.warnings.length > 0 && (
        <div className="diag-group">
          <div className="diag-group-title sev-warning">
            {groups.warnings.length} warning{groups.warnings.length === 1 ? "" : "s"}
          </div>
          {groups.warnings.map((d, i) => (
            <DiagRow key={`w${i}`} diag={d} onClick={() => onSelectDiag(d)} />
          ))}
        </div>
      )}

      {groups.advice.length > 0 && (
        <div className="diag-group">
          <button
            type="button"
            className="advice-toggle"
            onClick={() => setShowAdvice((v) => !v)}
          >
            {showAdvice ? "▾" : "▸"} {showAdvice ? "hide" : "show"} {groups.advice.length} advice
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
