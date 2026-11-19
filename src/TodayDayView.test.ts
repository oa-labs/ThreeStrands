import { describe, expect, it } from "vitest";
import { HOUR_HEIGHT } from "./calendarTime";
import { dayViewLayout } from "./TodayDayView";
import { workingSpans } from "./todayBooking";
import type { WorkingHours } from "./todayInsights";

const zone = Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC";
/** Wednesday 2026-10-28 is weekday 3. */
const hours = (...windows: [string, string][]): WorkingHours => ({ timeZone: zone, workingWindows: windows.map(([start, end]) => ({ weekday: 3, start, end })) });
const at = (h: number, m = 0) => new Date(2026, 9, 28, h, m);

describe("workingSpans", () => {
  it("gives the day's windows in local minutes, none on a day without them, and null when unknown", () => {
    expect(workingSpans(hours(["09:00", "12:00"], ["13:00", "17:30"]), at(8))).toEqual([{ startMinutes: 540, endMinutes: 720 }, { startMinutes: 780, endMinutes: 1050 }]);
    expect(workingSpans({ timeZone: zone, workingWindows: [{ weekday: 1, start: "09:00", end: "17:00" }] }, at(8))).toEqual([]);
    expect(workingSpans(null, at(8))).toBeNull();
  });
});

describe("dayViewLayout", () => {
  const nineToFive = hours(["09:00", "17:00"]);

  it("opens at the start of working hours before they begin and after they end", () => {
    expect(dayViewLayout(nineToFive, at(7, 30)).scrollMinutes).toBe(540);
    expect(dayViewLayout(nineToFive, at(9)).scrollMinutes).toBe(540);
    expect(dayViewLayout(nineToFive, at(17, 30)).scrollMinutes).toBe(540);
  });

  it("opens half an hour before now inside working hours, never above their start", () => {
    expect(dayViewLayout(nineToFive, at(14, 10)).scrollMinutes).toBe(13 * 60 + 40);
    expect(dayViewLayout(nineToFive, at(9, 10)).scrollMinutes).toBe(540);
  });

  it("sizes the grid to the full working day including gaps between split windows", () => {
    const split = dayViewLayout(hours(["09:00", "12:00"], ["13:00", "17:00"]), at(8));
    expect(split.heightPx).toBe(Math.round(8.5 * HOUR_HEIGHT));
    expect(split.scrollMinutes).toBe(540);
  });

  it("keeps the old behaviour when working hours are unknown or the day has none", () => {
    expect(dayViewLayout(null, at(10))).toEqual({ scrollMinutes: null, heightPx: null });
    expect(dayViewLayout({ timeZone: zone, workingWindows: [] }, at(10))).toEqual({ scrollMinutes: null, heightPx: null });
  });
});
