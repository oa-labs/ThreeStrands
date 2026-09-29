import type { AvailabilityCandidate } from "./domain";

/**
 * Builds the availability portion of a reply without involving an AI
 * provider. The ISO instants remain the source of truth; display formatting
 * is always performed in the user's selected IANA timezone.
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
    timeZoneName: "short",
  });
  const format = (value: string) => new Date(value);
  const lines = candidates.map((candidate) => {
    const start = format(candidate.start);
    const end = format(candidate.end);
    const startTime = timeFormatter.format(start);
    const endTime = timeFormatter.format(end);
    return `- ${dateFormatter.format(start)} · ${startTime}–${endTime} (${timeZone})`;
  });
  return `Here are some times that work for me:\n\n${lines.join("\n")}`;
}

/** Confirms one proposed time in a reply, formatted like the availability list. */
export function formatConfirmationText(slot: Pick<AvailabilityCandidate, "start" | "end">, timeZone: string): string {
  const [line] = formatAvailabilityText([{ ...slot, status: "verified" }], timeZone).split("\n").slice(-1);
  return `That time works for me: ${line.replace(/^- /, "")}.`;
}
