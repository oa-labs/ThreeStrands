import { describe, expect, it } from "vitest";
import { formatAvailabilityText } from "./actionDrafting";

describe("formatAvailabilityText", () => {
  it("preserves every selected instant and includes the timezone label", () => {
    const text = formatAvailabilityText([
      { start: "2026-09-22T13:00:00Z", end: "2026-09-22T13:30:00Z", status: "verified" },
      { start: "2026-09-23T14:00:00Z", end: "2026-09-23T14:45:00Z", status: "partiallyChecked" },
    ], "America/New_York");

    expect(text).toContain("Here are some times that work for me:");
    expect(text).toContain("September 22, 2026");
    expect(text).toContain("September 23, 2026");
    expect(text).toMatch(/9:00.*9:30/);
    expect(text).toContain("(America/New_York)");
    expect(text.split("\n").filter((line) => line.startsWith("- "))).toHaveLength(2);
  });
});
