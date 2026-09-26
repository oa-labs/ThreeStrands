import type { ScheduleResult } from "./domain";

export type ScheduleRequest = { timeMin: string; timeMax: string; timeZone: string };
// Retain complete ranges, including empty weeks, rather than truncating busy weeks.
export const MAX_CACHED_SCHEDULE_RANGES = 12;
const ranges = new Map<string, ScheduleResult>();
const pending = new Map<string, Promise<ScheduleResult>>();
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
      while (ranges.size > MAX_CACHED_SCHEDULE_RANGES) {
        ranges.delete(ranges.keys().next().value!);
      }
    }
    return result;
  }).finally(() => {
    if (pending.get(key) === promise) pending.delete(key);
  });
  pending.set(key, promise);
  return promise;
}
