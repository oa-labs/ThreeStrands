import type { ScheduleEvent, ScheduleResult } from "./domain";

export type ScheduleRequest = { timeMin: string; timeMax: string; timeZone: string };
// Retain complete ranges, including empty weeks, rather than truncating busy weeks.
export const MAX_CACHED_SCHEDULE_RANGES = 12;
const ranges = new Map<string, ScheduleResult>();
const pending = new Map<string, Promise<ScheduleResult>>();
// Ranges kept on screen after a local change but not yet confirmed by a refetch.
const stale = new Set<string>();
const listeners = new Set<() => void>();
let generation = 0;

export const scheduleCacheGeneration = () => generation;
export function subscribeScheduleCache(listener: () => void) {
  listeners.add(listener);
  return () => { listeners.delete(listener); };
}

export function clearScheduleCache() {
  generation += 1;
  ranges.clear();
  pending.clear();
  stale.clear();
  listeners.forEach((listener) => listener());
}

/**
 * Refetch every range after a change made from this app (an RSVP, edit, or new event)
 * while keeping cached events visible, so the grid does not blank out. Account or
 * calendar selection changes must use clearScheduleCache so other events never linger.
 */
export function revalidateScheduleCache(updated?: ScheduleEvent) {
  revalidate((events) => {
    if (!updated) return events;
    const replace = (event: ScheduleEvent) => sameScheduleEvent(event, updated);
    return events.some(replace) ? events.map((event) => replace(event) ? updated : event) : events;
  });
}

/** Like revalidateScheduleCache, but drops an event deleted from this app right away. */
export function removeFromScheduleCache(removed: ScheduleEvent) {
  revalidate((events) => events.some((event) => sameScheduleEvent(event, removed))
    ? events.filter((event) => !sameScheduleEvent(event, removed))
    : events);
}

const sameScheduleEvent = (left: ScheduleEvent, right: ScheduleEvent) =>
  left.id === right.id && left.accountId === right.accountId;

function revalidate(change: (events: ScheduleEvent[]) => ScheduleEvent[]) {
  generation += 1;
  pending.clear();
  for (const [key, result] of ranges) {
    stale.add(key);
    const events = change(result.events);
    if (events !== result.events) ranges.set(key, { ...result, events });
  }
  listeners.forEach((listener) => listener());
}

const keyFor = ({ timeMin, timeMax, timeZone }: ScheduleRequest) => JSON.stringify([timeMin, timeMax, timeZone]);

export function readScheduleCache(request: ScheduleRequest): ScheduleResult | undefined {
  const key = keyFor(request);
  const result = ranges.get(key);
  if (result) {
    ranges.delete(key);
    ranges.set(key, result);
  }
  return result;
}

export const isScheduleRangeFresh = (request: ScheduleRequest) =>
  ranges.has(keyFor(request)) && !stale.has(keyFor(request));

export function refreshScheduleCache(
  request: ScheduleRequest,
  load: () => Promise<ScheduleResult>,
): Promise<ScheduleResult> {
  const key = keyFor(request);
  const existing = pending.get(key);
  if (existing) return existing;
  const startedGeneration = generation;
  const promise = load().then((result) => {
    // Partial failures must not replace a complete cached schedule.
    if (generation === startedGeneration && result.errors.length === 0) {
      ranges.delete(key);
      ranges.set(key, result);
      stale.delete(key);
      while (ranges.size > MAX_CACHED_SCHEDULE_RANGES) {
        const oldest = ranges.keys().next().value!;
        ranges.delete(oldest);
        stale.delete(oldest);
      }
    }
    return result;
  }).finally(() => {
    if (pending.get(key) === promise) pending.delete(key);
  });
  pending.set(key, promise);
  return promise;
}
