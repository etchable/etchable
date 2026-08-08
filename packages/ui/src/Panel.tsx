import type { ElementType, ComponentPropsWithoutRef } from "react";

/** Quiet white surface: grid border, 16px radius, soft shadow. Panels
    never use copper. */
export function Panel<T extends ElementType = "div">({
  as,
  className,
  ...rest
}: { as?: T } & Omit<ComponentPropsWithoutRef<T>, "as">) {
  const Tag: ElementType = as ?? "div";
  return (
    <Tag
      className={`rounded-2xl border border-grid bg-white shadow-[0_2px_12px_rgba(33,36,46,0.06)] ${className ?? ""}`}
      {...rest}
    />
  );
}
