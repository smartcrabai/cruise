export const PHASE_ICON = {
  Completed: "[v]",
  Failed: "[x]",
  Suspended: "||",
} as const;

export function formatLocalTime(iso: string): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return "--";
  return date.toLocaleString(undefined, { dateStyle: "short", timeStyle: "short" });
}
