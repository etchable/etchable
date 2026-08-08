/** Named collaborator cursor ("you?" in copper, "us" in sky). */
export function Cursor({
  name,
  color,
  className,
}: {
  name: string;
  color: string;
  className?: string;
}) {
  return (
    <div className={`pointer-events-none absolute ${className ?? ""}`} aria-hidden>
      <svg width="20" height="20" viewBox="0 0 20 20">
        <path
          d="M3 1l5.5 15 2.2-6.3L17 7.5z"
          fill={color}
          stroke="#fff"
          strokeWidth="1.5"
        />
      </svg>
      <span
        className="ml-3 rounded-full px-2 py-0.5 font-mono text-xs text-white"
        style={{ backgroundColor: color }}
      >
        {name}
      </span>
    </div>
  );
}
