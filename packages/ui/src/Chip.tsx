import type { HTMLAttributes } from "react";

/** White rounded-full pill with a grid border — the "toolbar" register. */
export function Chip({ className, ...rest }: HTMLAttributes<HTMLDivElement>) {
  return (
    <div
      className={`inline-flex items-center gap-2 rounded-full border border-grid bg-white shadow-sm ${className ?? ""}`}
      {...rest}
    />
  );
}
