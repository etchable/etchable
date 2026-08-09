// Shimmer text: muted base copy with an ink-sweep overlay (the same
// .shimmer treatment the tool/reasoning triggers use while running).
// For "something is happening" labels — not for emphasis.

import type { ReactNode } from "react";
import { cn } from "../ui/utils";

export function ShimmerText({
  children,
  className,
}: {
  children: ReactNode;
  className?: string;
}) {
  return (
    <span
      className={cn(
        "relative inline-block text-xxs leading-none text-muted-foreground",
        className,
      )}
    >
      <span>{children}</span>
      <span aria-hidden className="shimmer pointer-events-none absolute inset-0 motion-reduce:animate-none">
        {children}
      </span>
    </span>
  );
}
