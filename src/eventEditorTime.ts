import { previewScheduleChoices } from "./scheduleTimes";

/** A wall-clock field in the selected display zone. The event keeps its instants. */
export function eventDateTimeInZone(at: number, timeZone: string): string {
  if (!Number.isFinite(at)) return "";
  const parts = new Intl.DateTimeFormat("en-US", { timeZone, year: "numeric", month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit", hourCycle: "h23" }).formatToParts(at);
  const part = (type: string) => parts.find((item) => item.type === type)?.value;
  return `${part("year")}-${part("month")}-${part("day")}T${part("hour")}:${part("minute")}`;
}

export function eventTimeInZone(value: string, timeZone: string, previous?: number): number {
  try {
    const choices = previewScheduleChoices(value, timeZone);
    // Retain the selected instant during the repeated hour when fields did not change.
    return choices.find((choice) => choice.scheduledAt === previous)?.scheduledAt ?? choices[0].scheduledAt;
  } catch { return NaN; }
}
