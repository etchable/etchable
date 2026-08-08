import type { ReactNode } from "react";

/** The hero device: marching-ants border + corner handles, as if the
    child were an object selected in its own editor. Use once per page,
    on the most important object. */
export function SelectionBox({ children }: { children: ReactNode }) {
  const handles = [
    "-top-[5px] -left-[5px]",
    "-top-[5px] -right-[5px]",
    "-bottom-[5px] -left-[5px]",
    "-bottom-[5px] -right-[5px]",
  ];
  return (
    <div className="relative inline-block px-8 py-4 sm:px-12 sm:py-6">
      <svg className="ants absolute inset-0 h-full w-full" aria-hidden>
        <rect x="1" y="1" width="calc(100% - 2px)" height="calc(100% - 2px)" rx="2" />
      </svg>
      {handles.map((pos) => (
        <span key={pos} className={`selection-handle ${pos}`} aria-hidden />
      ))}
      {children}
    </div>
  );
}
