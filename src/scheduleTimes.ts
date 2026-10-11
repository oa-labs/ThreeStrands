import type { ScheduleTimeChoice } from "./correspondence";

/** Browser preview only. Native code resolves and validates the actual send instant. */
export function previewScheduleChoices(localTime: string, timeZone: string): ScheduleTimeChoice[] {
  if (!/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}$/.test(localTime)) throw new Error("Choose a valid date and time");
  const format = new Intl.DateTimeFormat("sv-SE", { timeZone, year: "numeric", month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit", second: "2-digit", hourCycle: "h23" });
  const wall = (at: number) => {
    const parts = format.formatToParts(at);
    const part = (type: string) => parts.find((p) => p.type === type)?.value;
    return `${part("year")}-${part("month")}-${part("day")}T${part("hour")}:${part("minute")}:${part("second")}`;
  };
  const naive = Date.parse(`${localTime}:00Z`);
  if (!Number.isFinite(naive) || new Date(naive).toISOString().slice(0,16) !== localTime) throw new Error("Choose a valid date and time");
  const offsets = new Set([-86400000, 0, 86400000].map((delta) => (Date.parse(`${wall(naive + delta)}Z`) - (naive + delta)) / 1000));
  const choices = [...offsets].flatMap((offsetSeconds) => {
    const scheduledAt = naive - offsetSeconds * 1000;
    return wall(scheduledAt).slice(0,16) === localTime ? [{ scheduledAt, offsetSeconds }] : [];
  }).sort((a,b) => a.scheduledAt - b.scheduledAt);
  if (!choices.length) throw new Error("That time does not exist because the clocks change. Choose another time.");
  return choices;
}
