/** Small activity ring: grid track, copper sweep. */
export function Spinner({ className }: { className?: string }) {
  return (
    <span
      className={`inline-block size-2.5 flex-none animate-spin rounded-full border-[1.5px] border-grid border-t-copper ${className ?? ""}`}
      aria-label="loading"
    />
  );
}
