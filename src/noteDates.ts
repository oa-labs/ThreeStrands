/** A daily note's date as people read it: "Monday, November 9, 2026". */
export function formatNoteDate(date: string): string {
  const [year, month, day] = date.split("-").map(Number);
  return new Date(year, month - 1, day).toLocaleDateString(undefined, { weekday: "long", month: "long", day: "numeric", year: "numeric" });
}

/** How a notice names a day's note: "today's note", "tomorrow's note",
 * "yesterday's note", or "the note for Monday, November 17". */
export function dailyNoteName(date: string, now = new Date()): string {
  const [year, month, day] = date.split("-").map(Number);
  const target = new Date(year, month - 1, day);
  const today = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  const days = Math.round((target.getTime() - today.getTime()) / 86_400_000);
  if (days === 0) return "today's note";
  if (days === 1) return "tomorrow's note";
  if (days === -1) return "yesterday's note";
  return `the note for ${target.toLocaleDateString(undefined, { weekday: "long", month: "long", day: "numeric", ...(year === now.getFullYear() ? {} : { year: "numeric" }) })}`;
}
