import { describe, expect, it } from "vitest";
import { snoozePresets, toLocalDateTime } from "./snoozeTimes";

describe("snooze time choices", () => {
  it("offers future local morning times and next Monday", () => {
    const now = new Date(2026, 9, 31, 14, 20);
    const choices = snoozePresets(now);
    expect(choices.map((choice) => choice.label)).toEqual(["Later today", "Tomorrow", "Next week"]);
    expect(choices.every((choice) => choice.at > now)).toBe(true);
    expect(choices[1].at.getDate()).toBe(1);
    expect(choices[1].at.getHours()).toBe(9);
    expect(choices[2].at.getDay()).toBe(1);
    expect(choices[2].at.getHours()).toBe(9);
    expect(toLocalDateTime(choices[1].at)).toBe("2026-11-01T09:00");
  });
  it("never labels tomorrow as Later today, and Monday's next week is seven days away", () => {
    const now = new Date(2026, 10, 2, 23, 45);
    const choices = snoozePresets(now);
    expect(choices.map((choice) => choice.label)).toEqual(["Tomorrow", "Next week"]);
    expect(choices[1].at.getDate()).toBe(9);
  });
});
