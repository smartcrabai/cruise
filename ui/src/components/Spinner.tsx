export function Spinner({ color = "border-gray-400" }: { color?: string }) {
  return (
    <span
      role="status"
      aria-label="Loading"
      className={`inline-block w-3 h-3 rounded-full border-2 border-t-transparent animate-spin ${color}`}
    />
  );
}
