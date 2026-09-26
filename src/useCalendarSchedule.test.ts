import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  clearScheduleCache, MAX_CACHED_SCHEDULE_RANGES, readScheduleCache,
  refreshScheduleCache, type ScheduleRequest,
} from "./calendarScheduleCache";
import { mailClient } from "./data/client";
import type { ScheduleResult } from "./domain";
import { useCalendarSchedule } from "./useCalendarSchedule";

const week: ScheduleRequest = {
  timeMin: "2026-09-20T00:00:00.000Z", timeMax: "2026-09-27T00:00:00.000Z", timeZone: "UTC",
};
const next: ScheduleRequest = {
  ...week, timeMin: week.timeMax, timeMax: "2026-10-04T00:00:00.000Z",
};
function result(title = "Planning"): ScheduleResult {
  return { events: [{ id: "planning", accountId: "calendar@example.com", title,
    start: "2026-09-22T09:00:00Z", end: "2026-09-22T10:00:00Z", allDay: false }], errors: [] };
}
function deferred() {
  let resolve!: (value: ScheduleResult) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<ScheduleResult>((res, rej) => { resolve = res; reject = rej; });
  return { promise, resolve, reject };
}

describe("calendar schedule caching", () => {
  beforeEach(() => {
    clearScheduleCache();
    vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({ events: [], errors: [] });
    vi.spyOn(console, "error").mockImplementation(() => {});
  });
  afterEach(() => {
    cleanup();
    clearScheduleCache();
    vi.restoreAllMocks();
  });

  it("shows a revisited week's cache immediately and replaces it with refreshed events", async () => {
    const fresh = deferred();
    vi.mocked(mailClient.listScheduleEvents)
      .mockResolvedValueOnce(result()).mockResolvedValueOnce({ events: [], errors: [] })
      .mockReturnValueOnce(fresh.promise);
    const hook = renderHook((request) => useCalendarSchedule(request), { initialProps: week });
    await waitFor(() => expect(hook.result.current.events).toEqual(result().events));
    hook.rerender(next);
    await waitFor(() => expect(hook.result.current.loading).toBe(false));
    hook.rerender(week);
    expect(hook.result.current.events).toEqual(result().events);
    expect(hook.result.current.loading).toBe(false);
    await act(async () => { fresh.resolve(result("Updated")); });
    expect(hook.result.current.events[0].title).toBe("Updated");
  });

  it("retains cached events on a network failure and supports retry", async () => {
    await refreshScheduleCache(week, async () => result());
    vi.mocked(mailClient.listScheduleEvents).mockRejectedValueOnce(new Error("offline"));
    const hook = renderHook(() => useCalendarSchedule(week));
    await waitFor(() => expect(hook.result.current.error).not.toBeNull());
    expect(hook.result.current.events).toEqual(result().events);
    vi.mocked(mailClient.listScheduleEvents).mockResolvedValue(result("Recovered"));
    act(() => hook.result.current.reload());
    await waitFor(() => expect(hook.result.current.events[0].title).toBe("Recovered"));
    expect(hook.result.current.error).toBeNull();
  });

  it("keeps a complete cache when only some accounts refresh successfully", async () => {
    await refreshScheduleCache(week, async () => result());
    vi.mocked(mailClient.listScheduleEvents).mockResolvedValue({ events: [], errors: ["account offline"] });
    const hook = renderHook(() => useCalendarSchedule(week));
    await waitFor(() => expect(hook.result.current.error).not.toBeNull());
    expect(hook.result.current.events).toEqual(result().events);
    expect(readScheduleCache(week)).toEqual(result());
  });

  it("clears old events on uncached navigation and ignores out-of-order responses", async () => {
    const older = deferred();
    const newer = deferred();
    vi.mocked(mailClient.listScheduleEvents).mockReturnValueOnce(older.promise).mockReturnValueOnce(newer.promise);
    const hook = renderHook((request) => useCalendarSchedule(request), { initialProps: week });
    hook.rerender(next);
    await act(async () => { newer.resolve(result("Next week")); });
    await act(async () => { older.resolve(result("Old week")); });
    expect(hook.result.current.events[0].title).toBe("Next week");
    hook.rerender({ ...next, timeZone: "America/New_York" });
    expect(hook.result.current.events).toEqual([]);
  });

  it("preloads neighboring weeks and shares an in-flight prefetch with navigation", async () => {
    const neighbor = deferred();
    vi.mocked(mailClient.listScheduleEvents).mockImplementation((timeMin) =>
      timeMin === next.timeMin ? neighbor.promise : Promise.resolve(result()));
    const hook = renderHook((request) => useCalendarSchedule(request, true), { initialProps: week });
    await waitFor(() => expect(mailClient.listScheduleEvents).toHaveBeenCalledTimes(3));
    expect(vi.mocked(mailClient.listScheduleEvents).mock.calls.map((call) => call[0])).toEqual([
      week.timeMin, "2026-09-13T00:00:00.000Z", next.timeMin,
    ]);
    hook.rerender(next);
    expect(mailClient.listScheduleEvents).toHaveBeenCalledTimes(3);
    await act(async () => { neighbor.resolve(result("Neighbor")); });
    expect(hook.result.current.events[0].title).toBe("Neighbor");
  });

  it("invalidates mounted views and prevents old requests from repopulating the cache", async () => {
    await refreshScheduleCache(week, async () => result());
    const oldRequest = deferred();
    const newRequest = deferred();
    vi.mocked(mailClient.listScheduleEvents).mockReturnValueOnce(oldRequest.promise).mockReturnValueOnce(newRequest.promise);
    const hook = renderHook(() => useCalendarSchedule(week));
    expect(hook.result.current.events).toEqual(result().events);
    act(() => clearScheduleCache());
    expect(hook.result.current.events).toEqual([]);
    await act(async () => { oldRequest.resolve(result("Removed calendar")); });
    expect(readScheduleCache(week)).toBeUndefined();
    expect(hook.result.current.events).toEqual([]);
    await act(async () => { newRequest.resolve(result("Selected calendar")); });
    expect(hook.result.current.events[0].title).toBe("Selected calendar");
  });

  it("caches empty ranges and whole busy weeks, and evicts the least recently used range", async () => {
    const busy = result();
    busy.events = Array.from({ length: 101 }, (_, index) => ({ ...busy.events[0], id: String(index) }));
    await refreshScheduleCache(week, async () => busy);
    expect(readScheduleCache(week)?.events).toHaveLength(101);
    const requests = Array.from({ length: MAX_CACHED_SCHEDULE_RANGES }, (_, index) => ({ ...week, timeZone: `zone-${index}` }));
    for (const request of requests.slice(0, -1)) {
      await refreshScheduleCache(request, async () => ({ events: [], errors: [] }));
    }
    expect(readScheduleCache(week)?.events).toHaveLength(101);
    await refreshScheduleCache(requests.at(-1)!, async () => ({ events: [], errors: [] }));
    expect(readScheduleCache(requests[0])).toBeUndefined();
    expect(readScheduleCache(requests.at(-1)!)).toEqual({ events: [], errors: [] });
    expect(readScheduleCache(week)?.events).toHaveLength(101);
  });
});
