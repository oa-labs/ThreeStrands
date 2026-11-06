// Open times found for a search, kept briefly so a card that unmounts (for
// example while the user looks at the week) shows them again at once. The
// card still re-checks in the background; the cache only hides the wait.

import type { AvailabilityResult } from "./domain";
import { subscribeScheduleCache } from "./calendarScheduleCache";
import { CALENDAR_AVAILABILITY_CHANGED } from "./calendarAvailability";

/** How long found times may be shown before a re-check finishes. */
export const AVAILABILITY_CACHE_MAX_AGE_MS = 10 * 60_000;
/** Searches remembered at once; the least recently used is dropped first. */
export const MAX_CACHED_AVAILABILITY_SEARCHES = 20;

/** What the user did with the times: which to offer, which day is open, and how many show. */
export type AvailabilityView = { selected: string[]; previewing: string | null; visibleCount: number };

type Entry = { result: AvailabilityResult; storedAt: number; view: AvailabilityView | null };

const entries = new Map<string, Entry>();

export function clearAvailabilityCache() {
  entries.clear();
}

// A calendar change or account switch can make any found time wrong.
subscribeScheduleCache(clearAvailabilityCache);
if (typeof window !== "undefined") window.addEventListener(CALENDAR_AVAILABILITY_CHANGED, clearAvailabilityCache);

/** Identifies a search by everything sent to find it. */
export const availabilityKey = (request: unknown) => JSON.stringify(request);

/** The times found for a search, unless they are too old to show. */
export function readAvailability(key: string, now = Date.now()): { result: AvailabilityResult; view: AvailabilityView | null } | null {
  const entry = entries.get(key);
  if (!entry) return null;
  if (now - entry.storedAt > AVAILABILITY_CACHE_MAX_AGE_MS) {
    entries.delete(key);
    return null;
  }
  entries.delete(key);
  entries.set(key, entry);
  return { result: entry.result, view: entry.view };
}

/** Keeps a search's times, and the view on them, which a new result keeps. */
export function storeAvailability(key: string, result: AvailabilityResult, now = Date.now()) {
  const view = entries.get(key)?.view ?? null;
  entries.delete(key);
  entries.set(key, { result, storedAt: now, view });
  while (entries.size > MAX_CACHED_AVAILABILITY_SEARCHES) entries.delete(entries.keys().next().value!);
}

/** Remembers the view on a search's times, without making them any newer. */
export function rememberAvailabilityView(key: string, view: AvailabilityView) {
  const entry = entries.get(key);
  if (entry) entry.view = view;
}

/** Whether a re-check found the same times, so the shown ones can stay. */
export function sameAvailability(left: AvailabilityResult, right: AvailabilityResult): boolean {
  return JSON.stringify(left) === JSON.stringify(right);
}
