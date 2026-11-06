import { afterEach, describe, expect, it } from "vitest";
import {
  AVAILABILITY_CACHE_MAX_AGE_MS, availabilityKey, clearAvailabilityCache, MAX_CACHED_AVAILABILITY_SEARCHES, readAvailability,
  rememberAvailabilityView, sameAvailability, storeAvailability,
} from "./availabilityCache";
import { CALENDAR_AVAILABILITY_CHANGED } from "./calendarAvailability";
import { clearScheduleCache, revalidateScheduleCache } from "./calendarScheduleCache";
import type { AvailabilityResult } from "./domain";

const result = (start: string): AvailabilityResult => ({
  candidates: [{ start, end: start, status: "verified" }], checkedCalendarCount: 1, totalCalendarCount: 1, uncheckedCalendars: [], errors: [],
});
const key = availabilityKey({ rangeStart: "a", rangeEnd: "b" });

describe("availability cache", () => {
  afterEach(clearAvailabilityCache);

  it("shows found times until just past the age limit", () => {
    storeAvailability(key, result("x"), 0);
    expect(readAvailability(key, AVAILABILITY_CACHE_MAX_AGE_MS - 1)?.result).toEqual(result("x"));
    expect(readAvailability(key, AVAILABILITY_CACHE_MAX_AGE_MS)?.result).toEqual(result("x"));
    expect(readAvailability(key, AVAILABILITY_CACHE_MAX_AGE_MS + 1)).toBeNull();
    // An expired search is gone, not just hidden.
    expect(readAvailability(key, 0)).toBeNull();
  });

  it("keeps up to the limit of searches, dropping the least recently used", () => {
    const keys = Array.from({ length: MAX_CACHED_AVAILABILITY_SEARCHES + 1 }, (_, index) => availabilityKey({ index }));
    keys.slice(0, MAX_CACHED_AVAILABILITY_SEARCHES).forEach((each) => storeAvailability(each, result(each), 0));
    // Reading the oldest makes it the most recent.
    expect(readAvailability(keys[0], 0)).not.toBeNull();
    storeAvailability(keys[MAX_CACHED_AVAILABILITY_SEARCHES], result("new"), 0);
    expect(readAvailability(keys[0], 0)).not.toBeNull();
    expect(readAvailability(keys[1], 0)).toBeNull();
    expect(readAvailability(keys[MAX_CACHED_AVAILABILITY_SEARCHES], 0)).not.toBeNull();
  });

  it("keeps the view on a search's times across a new result for it, without making them newer", () => {
    const view = { selected: ["x"], previewing: "x", visibleCount: 3 };
    rememberAvailabilityView(key, view);
    // Nothing is kept for a search never found.
    expect(readAvailability(key, 0)).toBeNull();
    storeAvailability(key, result("x"), 0);
    rememberAvailabilityView(key, view);
    storeAvailability(key, result("y"), 10);
    expect(readAvailability(key, 10)).toEqual({ result: result("y"), view });
    rememberAvailabilityView(key, { ...view, visibleCount: 6 });
    expect(readAvailability(key, 10 + AVAILABILITY_CACHE_MAX_AGE_MS + 1)).toBeNull();
  });

  it.each([
    ["the calendar choices change", () => window.dispatchEvent(new Event(CALENDAR_AVAILABILITY_CHANGED))],
    ["accounts or calendars switch", () => clearScheduleCache()],
    ["an event changes in the app", () => revalidateScheduleCache()],
  ])("forgets every search when %s", (_, change) => {
    storeAvailability(key, result("x"));
    change();
    expect(readAvailability(key)).toBeNull();
  });

  it("tells a re-check that found the same times from one that did not", () => {
    expect(sameAvailability(result("x"), result("x"))).toBe(true);
    expect(sameAvailability(result("x"), result("y"))).toBe(false);
    expect(sameAvailability(result("x"), { ...result("x"), uncheckedCalendars: ["Team"] })).toBe(false);
  });
});
