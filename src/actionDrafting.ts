import type { AvailabilityCandidate } from "./domain";

/**
 * Builds the availability portion of a reply without involving an AI
 * provider. The ISO instants remain the source of truth; display formatting
 * is always performed in the user's selected IANA timezone, labeled with the
 * familiar abbreviation in effect at each slot (EDT, PST) rather than the
 * IANA identifier.
 */
export function formatAvailabilityText(
  candidates: AvailabilityCandidate[],
  timeZone: string,
): string {
  const dateFormatter = new Intl.DateTimeFormat(undefined, {
    weekday: "long",
    month: "long",
    day: "numeric",
    year: "numeric",
    timeZone,
  });
  const timeFormatter = new Intl.DateTimeFormat(undefined, {
    hour: "numeric",
    minute: "2-digit",
    timeZone,
  });
  // en-US carries the widely recognized North American abbreviations; zones
  // without one fall back to a GMT offset such as "GMT+2".
  const zoneFormatter = new Intl.DateTimeFormat("en-US", { timeZone, timeZoneName: "short" });
  const zoneName = (date: Date) =>
    zoneFormatter.formatToParts(date).find((part) => part.type === "timeZoneName")?.value ?? timeZone;
  const format = (value: string) => new Date(value);
  const lines = candidates.map((candidate) => {
    const start = format(candidate.start);
    const end = format(candidate.end);
    const startZone = zoneName(start);
    const endZone = zoneName(end);
    const startTime = timeFormatter.format(start);
    const endTime = timeFormatter.format(end);
    const range = startZone === endZone
      ? `${startTime}–${endTime} ${endZone}`
      : `${startTime} ${startZone}–${endTime} ${endZone}`;
    return `- ${dateFormatter.format(start)} · ${range}`;
  });
  return `Here are some times that work for me:\n\n${lines.join("\n")}`;
}

/** Confirms one proposed time in a reply, formatted like the availability list. */
export function formatConfirmationText(slot: Pick<AvailabilityCandidate, "start" | "end">, timeZone: string): string {
  const [line] = formatAvailabilityText([{ ...slot, status: "verified" }], timeZone).split("\n").slice(-1);
  return `That time works for me: ${line.replace(/^- /, "")}.`;
}
