import { describe, expect, it } from "vitest";
import { schedulingConflicts, schedulingDay, schedulingDayStart, schedulingWeek, shiftSchedulingDay } from "./meetingAvailability";
import type { MeetingAvailabilityResult } from "./domain";

describe("meeting availability", () => {
  it("uses the scheduling zone's day rather than the device's date", () => {
    expect(schedulingDay(Date.parse("2026-10-27T02:00:00Z"), "America/New_York")).toBe("2026-10-26");
    expect(schedulingWeek("2026-11-01")).toBe("2026-10-26");
    expect(shiftSchedulingDay("2026-12-31", 1)).toBe("2027-01-01");
  });

  it.each([["2026-03-08", 23], ["2026-11-01", 25]])("keeps the actual length of the DST day %s", (day, hours) => {
    expect((schedulingDayStart(shiftSchedulingDay(day, 1), "America/New_York") - schedulingDayStart(day, "America/New_York")) / 3_600_000).toBe(hours);
  });

  it("resolves a day whose midnight is skipped", () => {
    expect(new Date(schedulingDayStart("2026-03-08", "America/Havana")).toISOString()).toBe("2026-03-08T05:00:00.000Z");
  });

  it("keeps identities for overlapping conflicts and allows touching boundaries", () => {
    const result: MeetingAvailabilityResult = { own: { email: "me@example.com", checked: true, busy: [] }, guests: [
      { email: "a@example.com", checked: true, busy: [{ start: "2026-10-26T13:00:00Z", end: "2026-10-26T14:00:00Z" }] },
      { email: "b@example.com", checked: false, busy: [] },
    ], candidates: [] };
    expect(schedulingConflicts(result, Date.parse("2026-10-26T13:30:00Z"), Date.parse("2026-10-26T14:30:00Z")).map((person) => person.email)).toEqual(["a@example.com"]);
    expect(schedulingConflicts(result, Date.parse("2026-10-26T14:00:00Z"), Date.parse("2026-10-26T14:30:00Z"))).toEqual([]);
  });
});
