// Recognizes when a suggested event is already on the user's calendar. An
// event ThreeStrands added from the same email carries that email's id and is
// recognized first; otherwise the event's own title, time, and place decide.
// Matching is conservative: a title and time that agree are the same event; a
// shared place at the same time is only a possibility; anything less stays an
// ordinary conflict.

import type { ScheduleEvent } from "./domain";

/** How far apart two starts can be and still be the same event, for calendars entered by hand. */
export const SAME_EVENT_START_TOLERANCE_MINUTES = 15;
/** The share of the shorter title's words the longer one must contain. */
export const TITLE_SHORTER_COVERAGE = 0.8;
/** The share of the longer title's words the shorter one must account for. */
export const TITLE_LONGER_COVERAGE = 0.5;
/** Words two locations must share. */
export const LOCATION_MIN_SHARED_WORDS = 3;
/** The share of each location's words the other must contain, so a shared city and state are not enough. */
export const LOCATION_COVERAGE = 0.6;

/** What a suggestion is called, where it is, and the email it was read from, compared with the calendar. */
export type EventIdentity = { title: string; location?: string | null; sourceMessageId?: string | null };

export type CalendarMatch = {
  /** `same` when title and time agree; `maybe` when only the place and time do. */
  kind: "same" | "maybe";
  event: ScheduleEvent;
};

/**
 * An event added from the same email that also agrees with the suggestion on
 * its title or its time, so an email announcing two events never marks the
 * second added. A title match outranks a time match, then the closer start.
 */
function findTaggedMatch(
  identity: EventIdentity,
  events: readonly ScheduleEvent[],
  agreesOnTime: (event: ScheduleEvent) => boolean,
  distance: (event: ScheduleEvent) => number,
): CalendarMatch | null {
  const source = identity.sourceMessageId;
  if (!source) return null;
  const ranked = events
    .filter((event) => event.sourceMessageId === source)
    .map((event) => ({ event, titled: titlesMatch(identity.title, event.title) }))
    .filter(({ event, titled }) => titled || agreesOnTime(event))
    .sort((left, right) => Number(right.titled) - Number(left.titled) || distance(left.event) - distance(right.event));
  return ranked[0] ? { kind: "same", event: ranked[0].event } : null;
}

// Words invitations and calendars wrap around an event's name.
const TITLE_NOISE = new Set([
  "a", "an", "and", "at", "for", "in", "of", "on", "the", "to", "with",
  "you", "you're", "youre", "your", "re", "fw", "fwd", "invitation", "invited", "invite", "updated", "reminder", "rsvp", "join", "us",
]);

function words(text: string, noise: ReadonlySet<string> = new Set()): Set<string> {
  return new Set(text.normalize("NFKD").replace(/[̀-ͯ]/g, "").toLowerCase()
    .replace(/['’]/g, "")
    .split(/[^\p{L}\p{N}]+/u)
    .filter((word) => word && !noise.has(word)));
}

function shared(left: ReadonlySet<string>, right: ReadonlySet<string>): number {
  let count = 0;
  for (const word of left) if (right.has(word)) count += 1;
  return count;
}

/** Whether two titles name the same event, allowing wrapper words and an added year or qualifier. */
export function titlesMatch(left: string, right: string): boolean {
  const a = words(left, TITLE_NOISE);
  const b = words(right, TITLE_NOISE);
  const [shorter, longer] = a.size <= b.size ? [a, b] : [b, a];
  if (shorter.size === 0) return false;
  const overlap = shared(shorter, longer);
  // One word ("Meeting", "Lunch") is too generic to match on anything less than the whole title.
  if (shorter.size === 1) return longer.size === 1 && overlap === 1;
  return overlap / shorter.size >= TITLE_SHORTER_COVERAGE && overlap / longer.size >= TITLE_LONGER_COVERAGE;
}

/**
 * Whether two locations name the same place: mostly the same words, including
 * a street or room number, so neighbors on one street or in one city differ.
 */
export function locationsMatch(left: string | null | undefined, right: string | null | undefined): boolean {
  if (!left?.trim() || !right?.trim()) return false;
  const a = words(left);
  const b = words(right);
  const overlap = shared(a, b);
  const sharesNumber = [...a].some((word) => /\d/.test(word) && b.has(word));
  return sharesNumber && overlap >= LOCATION_MIN_SHARED_WORDS
    && overlap / a.size >= LOCATION_COVERAGE && overlap / b.size >= LOCATION_COVERAGE;
}

/**
 * The calendar event a timed suggestion already appears as, if any. An event
 * added from the same email counts at any time the events include; otherwise
 * only timed events starting within the tolerance are considered, a title
 * match outranks a place match, and a closer start breaks ties.
 */
export function findCalendarMatch(
  identity: EventIdentity,
  slot: { start: string; end: string },
  events: readonly ScheduleEvent[],
): CalendarMatch | null {
  const start = Date.parse(slot.start);
  const tolerance = SAME_EVENT_START_TOLERANCE_MINUTES * 60_000;
  const distance = (event: ScheduleEvent) => event.allDay ? Infinity : Math.abs(Date.parse(event.start) - start);
  const tagged = findTaggedMatch(identity, events, (event) => distance(event) <= tolerance, distance);
  if (tagged) return tagged;
  let best: (CalendarMatch & { distance: number }) | null = null;
  for (const event of events) {
    const gap = distance(event);
    if (!(gap <= tolerance)) continue;
    const kind = titlesMatch(identity.title, event.title) ? "same"
      : locationsMatch(identity.location, event.location) ? "maybe" : null;
    if (!kind) continue;
    const better = !best || (kind === "same" && best.kind === "maybe") || (kind === best.kind && gap < best.distance);
    if (better) best = { kind, event, distance: gap };
  }
  return best ? { kind: best.kind, event: best.event } : null;
}

/** An event on a `YYYY-MM-DD` day with the suggestion's title, all-day or timed. */
export function findDayMatch(identity: EventIdentity, day: string, events: readonly ScheduleEvent[]): CalendarMatch | null {
  const [year, month, date] = day.split("-").map(Number);
  const dayStart = new Date(year, month - 1, date).getTime();
  const dayEnd = new Date(year, month - 1, date + 1).getTime();
  const onDay = (event: ScheduleEvent) => event.allDay
    // All-day events carry inclusive-start, exclusive-end `YYYY-MM-DD` days.
    ? event.start.slice(0, 10) <= day && day < event.end.slice(0, 10)
    : Date.parse(event.start) < dayEnd && Date.parse(event.end) > dayStart;
  const tagged = findTaggedMatch(identity, events.filter(onDay), () => true, () => 0);
  if (tagged) return tagged;
  const event = events.find((candidate) => onDay(candidate) && titlesMatch(identity.title, candidate.title));
  return event ? { kind: "same", event } : null;
}
