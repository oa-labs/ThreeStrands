/** Presets use local wall-clock days, so DST changes preserve the morning hour. */
export function snoozePresets(now: Date): { label: string; at: Date }[] {
  const later = new Date(now.getTime() + 3 * 60 * 60 * 1000);
  const tomorrow = new Date(now);
  tomorrow.setDate(tomorrow.getDate() + 1);
  tomorrow.setHours(9, 0, 0, 0);
  const nextWeek = new Date(now);
  nextWeek.setDate(nextWeek.getDate() + ((8 - nextWeek.getDay()) % 7 || 7));
  nextWeek.setHours(9, 0, 0, 0);
  return [...(later.getDate() === now.getDate() ? [{ label: "Later today", at: later }] : []), { label: "Tomorrow", at: tomorrow }, { label: "Next week", at: nextWeek }];
}

export function toLocalDateTime(date: Date): string {
  const part = (value: number) => String(value).padStart(2, "0");
  return `${date.getFullYear()}-${part(date.getMonth() + 1)}-${part(date.getDate())}T${part(date.getHours())}:${part(date.getMinutes())}`;
}
