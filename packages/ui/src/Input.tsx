import { forwardRef } from "react";
import type { InputHTMLAttributes } from "react";

export type InputVariant = "pill" | "field";
export type InputSize = "sm" | "md" | "lg";

const VARIANT: Record<InputVariant, string> = {
  // The landing page's pill input.
  pill: "rounded-full border-2 border-ink/15",
  // Softer rectangle for stacked forms (auth panel).
  field: "rounded-xl border-2 border-ink/10",
};

const SIZE: Record<InputSize, string> = {
  sm: "px-3 py-1 text-xs",
  md: "px-4 py-2.5",
  lg: "px-5 py-3",
};

export type InputProps = InputHTMLAttributes<HTMLInputElement> & {
  variant?: InputVariant;
  inputSize?: InputSize;
  mono?: boolean;
};

/** Forwards its ref: callers need the element to focus or select it (the
 *  application menu opens the new-project form and puts the cursor in it). */
export const Input = forwardRef<HTMLInputElement, InputProps>(function Input(
  { variant = "pill", inputSize = "lg", mono = false, className, ...rest },
  ref,
) {
  return (
    <input
      ref={ref}
      className={`min-w-0 bg-white text-ink placeholder-ink-soft/60 outline-none transition select-text focus:border-sky ${VARIANT[variant]} ${SIZE[inputSize]} ${mono ? "font-mono" : ""} ${className ?? ""}`}
      {...rest}
    />
  );
});
