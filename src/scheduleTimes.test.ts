import { describe, expect, it } from "vitest";
import { previewScheduleChoices } from "./scheduleTimes";

describe("schedule time resolution in browser preview", () => {
  it("rejects missing times and invalid calendar input", () => {
    expect(() => previewScheduleChoices("2026-03-08T02:30", "America/New_York")).toThrow(/does not exist/);
    expect(() => previewScheduleChoices("2026-02-30T12:00", "UTC")).toThrow();
    expect(() => previewScheduleChoices("invalid", "UTC")).toThrow();
    expect(() => previewScheduleChoices("2026-10-10T12:00", "invalid")).toThrow();
  });
  it("offers both repeated times and supports quarter-hour zones", () => {
    const repeated = previewScheduleChoices("2026-11-01T01:30", "America/New_York");
    expect(repeated.map((v) => v.offsetSeconds)).toEqual([-14400, -18000]);
    expect(repeated[1].scheduledAt - repeated[0].scheduledAt).toBe(3600000);
    const nepal = previewScheduleChoices("2026-10-10T12:00", "Asia/Kathmandu");
    expect(nepal).toEqual([{ scheduledAt: Date.parse("2026-10-10T06:15:00Z"), offsetSeconds: 20700 }]);
  });
});
