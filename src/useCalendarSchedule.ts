import { useCallback, useEffect, useState, useSyncExternalStore } from "react";
import { addDays } from "./calendarTime";
import {
  isScheduleRangeFresh, readScheduleCache, refreshScheduleCache, scheduleCacheGeneration,
  subscribeScheduleCache, type ScheduleRequest,
} from "./calendarScheduleCache";
import { mailClient } from "./data/client";
import type { ScheduleResult } from "./domain";

export const CALENDAR_POLL_INTERVAL_MS = 30_000;
export const CALENDAR_REFRESH_COOLDOWN_MS = 5_000;

const fetchSchedule = (request: ScheduleRequest, cooldownMs = 0) => refreshScheduleCache(request, () =>
  mailClient.listScheduleEvents(request.timeMin, request.timeMax, request.timeZone), cooldownMs);

export function useCalendarSchedule({ timeMin, timeMax, timeZone }: ScheduleRequest, prefetchWeeks: boolean | number = false) {
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
    let retainedResult = previous;
    let refreshing = false;
    let lastRefresh = -Infinity;
    const load = (automatic = false) => {
      if (refreshing || (automatic && Date.now() - lastRefresh < CALENDAR_REFRESH_COOLDOWN_MS)) return;
      refreshing = true;
      lastRefresh = Date.now();
      void fetchSchedule(request, automatic ? CALENDAR_REFRESH_COOLDOWN_MS : 0).then((result) => {
        if (!active || generation !== scheduleCacheGeneration()) return;
        const failed = result.errors.length > 0;
        if (failed) console.error("Calendar schedule load failed:", result.errors);
        const retained = readScheduleCache(request) ?? retainedResult;
        retainedResult = failed && retained ? retained : result;
        setState({ key, result: retainedResult, loading: false,
          error: failed ? "Calendar schedule load failed" : null });
        if (!automatic && prefetchWeeks && !failed) {
          for (const offset of typeof prefetchWeeks === "number" ? [-prefetchWeeks, prefetchWeeks] : [-7, 7]) {
            const neighbor = { timeMin: addDays(new Date(timeMin), offset).toISOString(),
              timeMax: addDays(new Date(timeMax), offset).toISOString(), timeZone };
            // A speculative fetch failure should not mark the visible week as failed.
            if (!isScheduleRangeFresh(neighbor)) void fetchSchedule(neighbor).catch(() => {});
          }
        }
      }).catch((reason: unknown) => {
        if (!active || generation !== scheduleCacheGeneration()) return;
        console.error("Calendar schedule load failed:", reason);
        setState({ key, result: readScheduleCache(request) ?? retainedResult, loading: false, error: "Calendar schedule load failed" });
      }).finally(() => { refreshing = false; });
    };
    load();
    // Mounted schedule consumers are the visible calendar, agenda, and meeting views.
    // Treat an unfocused desktop window as background even if it is still visible.
    let foreground = document.visibilityState === "visible" && document.hasFocus();
    let timer: number | undefined;
    const stop = () => { window.clearInterval(timer); timer = undefined; };
    const start = () => {
      stop();
      timer = window.setInterval(() => {
        if (document.visibilityState === "visible" && document.hasFocus()) load(true);
      }, CALENDAR_POLL_INTERVAL_MS);
    };
    const resume = () => {
      if (document.visibilityState !== "visible" || !document.hasFocus()) return;
      if (foreground) return;
      foreground = true;
      load(true);
      start();
    };
    const pause = () => { foreground = false; stop(); };
    const visibilityChanged = () => {
      if (document.visibilityState === "hidden") pause();
      else resume();
    };
    if (foreground) start();
    window.addEventListener("focus", resume);
    window.addEventListener("blur", pause);
    document.addEventListener("visibilitychange", visibilityChanged);
    return () => {
      active = false;
      stop();
      window.removeEventListener("focus", resume);
      window.removeEventListener("blur", pause);
      document.removeEventListener("visibilitychange", visibilityChanged);
    };
  }, [timeMin, timeMax, timeZone, generation, retry, key, prefetchWeeks]);

  // Never render the preceding range while the effect for a new range is pending.
  const current = state.key === key ? state : { result: cached, loading: !cached, error: null };
  const reload = useCallback(() => setRetry((value) => value + 1), []);
  return { events: current.result?.events ?? [], loading: current.loading, error: current.error, reload };
}
