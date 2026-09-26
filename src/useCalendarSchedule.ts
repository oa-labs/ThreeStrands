import { useCallback, useEffect, useState, useSyncExternalStore } from "react";
import { addDays } from "./calendarTime";
import {
  readScheduleCache, refreshScheduleCache, scheduleCacheGeneration,
  subscribeScheduleCache, type ScheduleRequest,
} from "./calendarScheduleCache";
import { mailClient } from "./data/client";
import type { ScheduleResult } from "./domain";

const fetchSchedule = (request: ScheduleRequest) => refreshScheduleCache(request, () =>
  mailClient.listScheduleEvents(request.timeMin, request.timeMax, request.timeZone));

export function useCalendarSchedule({ timeMin, timeMax, timeZone }: ScheduleRequest, prefetchWeeks = false) {
  const generation = useSyncExternalStore(subscribeScheduleCache, scheduleCacheGeneration);
  const [retry, setRetry] = useState(0);
  const [state, setState] = useState<{
    key: string; result?: ScheduleResult; loading: boolean; error: string | null;
  }>({ key: "", loading: true, error: null });
  const key = JSON.stringify([timeMin, timeMax, timeZone, generation, retry]);
  const cached = readScheduleCache({ timeMin, timeMax, timeZone });

  useEffect(() => {
    let active = true;
    const request = { timeMin, timeMax, timeZone };
    const previous = readScheduleCache(request);
    setState({ key, result: previous, loading: !previous, error: null });
    void fetchSchedule(request).then((result) => {
      if (!active || generation !== scheduleCacheGeneration()) return;
      const failed = result.errors.length > 0;
      if (failed) console.error("Calendar schedule load failed:", result.errors);
      setState({ key, result: failed && previous ? previous : result, loading: false,
        error: failed ? "Calendar schedule load failed" : null });
      if (prefetchWeeks && !failed) {
        for (const offset of [-7, 7]) {
          const neighbor = { timeMin: addDays(new Date(timeMin), offset).toISOString(),
            timeMax: addDays(new Date(timeMax), offset).toISOString(), timeZone };
          // A speculative fetch failure should not mark the visible week as failed.
          if (!readScheduleCache(neighbor)) void fetchSchedule(neighbor).catch(() => {});
        }
      }
    }).catch((reason: unknown) => {
      if (!active || generation !== scheduleCacheGeneration()) return;
      console.error("Calendar schedule load failed:", reason);
      setState({ key, result: previous, loading: false, error: "Calendar schedule load failed" });
    });
    return () => { active = false; };
  }, [timeMin, timeMax, timeZone, generation, retry, key, prefetchWeeks]);

  // Never render the preceding range while the effect for a new range is pending.
  const current = state.key === key ? state : { result: cached, loading: !cached, error: null };
  const reload = useCallback(() => setRetry((value) => value + 1), []);
  return { events: current.result?.events ?? [], loading: current.loading, error: current.error, reload };
}
