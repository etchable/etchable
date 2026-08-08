import type { ButtonHTMLAttributes } from "react";

export type ButtonVariant = "copper" | "ink" | "quiet";
export type ButtonSize = "sm" | "md" | "lg";

const VARIANT: Record<ButtonVariant, string> = {
  // Copper is the etch: it appears where the user acts. Hard pressed
  // shadow that compresses on press — tactile, tool-like.
  copper:
    "bg-copper font-bold text-white " +
    "shadow-[0_3px_0_var(--color-copper-deep)] " +
    "hover:-translate-y-0.5 hover:shadow-[0_5px_0_var(--color-copper-deep)] " +
    "active:translate-y-0 active:shadow-[0_2px_0_var(--color-copper-deep)]",
  ink: "bg-ink font-bold text-white hover:bg-ink/85",
  quiet: "border border-grid bg-white font-medium text-ink shadow-sm hover:border-sky",
};

const SIZE: Record<ButtonSize, string> = {
  sm: "px-3 py-1 text-xs",
  md: "px-4 py-1.5 text-sm",
  lg: "px-6 py-3",
};

export type ButtonTone = "success" | "danger";

// Tones recolor the quiet variant for confirm/destroy pairs (Allow/Deny,
// Stop). Copper and ink don't take tones — they already are one.
const TONE: Record<ButtonTone, string> = {
  success: "text-[#1f8f53] border-leaf/50",
  danger: "text-alert border-alert/45",
};

export type ButtonProps = ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: ButtonVariant;
  size?: ButtonSize;
  tone?: ButtonTone;
};

export function Button({
  variant = "quiet",
  size = "md",
  tone,
  className,
  type = "button",
  ...rest
}: ButtonProps) {
  return (
    <button
      type={type}
      className={`rounded-full whitespace-nowrap transition disabled:pointer-events-none disabled:opacity-50 ${VARIANT[variant]} ${SIZE[size]} ${tone ? TONE[tone] : ""} ${className ?? ""}`}
      {...rest}
    />
  );
}
