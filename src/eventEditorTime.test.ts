import { describe, expect, it } from "vitest";
import { eventDateTimeInZone, eventTimeInZone } from "./eventEditorTime";

describe("event editor time zones", () => {
  it("converts instants across calendar days and non-hour offsets", () => {
    const at = Date.parse("2026-11-12T01:00:00Z");
    expect(eventDateTimeInZone(at, "America/Los_Angeles")).toBe("2026-11-11T17:00");
    expect(eventDateTimeInZone(at, "Asia/Kolkata")).toBe("2026-11-12T06:30");
    expect(eventTimeInZone("2026-11-12T06:30", "Asia/Kolkata")).toBe(at);
  });

  it("refuses nonexistent spring times and invalid dates instead of moving them", () => {
    expect(eventTimeInZone("2026-03-08T02:30", "America/New_York")).toBeNaN();
    expect(eventTimeInZone("2026-02-30T09:00", "UTC")).toBeNaN();
    expect(eventTimeInZone("2026-11-12T", "UTC")).toBeNaN();
  });

  it("retains either occurrence of the repeated fall hour", () => {
    for (const offset of ["-04:00", "-05:00"]) {
      const at = Date.parse(`2026-11-01T01:30:00${offset}`);
      expect(eventDateTimeInZone(at, "America/New_York")).toBe("2026-11-01T01:30");
      expect(eventTimeInZone("2026-11-01T01:30", "America/New_York", at)).toBe(at);
    }
  });
});
