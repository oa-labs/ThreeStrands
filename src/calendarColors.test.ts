import { afterEach, describe, expect, it } from "vitest";
import {
  CALENDAR_COLORS,
  CALENDAR_COLORS_KEY,
  calendarColorStyle,
  readCalendarColors,
  resetCalendarColorsForTests,
  setCalendarColor,
} from "./calendarColors";

describe("calendar colors", () => {
  afterEach(() => {
    localStorage.clear();
    resetCalendarColorsForTests();
  });

  it("offers sixteen distinct hex colors", () => {
    expect(CALENDAR_COLORS).toHaveLength(16);
    expect(new Set(CALENDAR_COLORS.map((color) => color.id)).size).toBe(16);
    expect(new Set(CALENDAR_COLORS.map((color) => color.value)).size).toBe(16);
    for (const color of CALENDAR_COLORS) expect(color.value).toMatch(/^#[0-9a-f]{6}$/);
  });

  it("stores a palette id per account and calendar", () => {
    setCalendarColor("joel@example.com", "primary", "teal");
    setCalendarColor("joel@example.com", "team", "red");
    setCalendarColor("other@example.com", "primary", "gray");

    expect(JSON.parse(localStorage.getItem(CALENDAR_COLORS_KEY)!)).toEqual({
      "joel@example.com": { primary: "teal", team: "red" },
      "other@example.com": { primary: "gray" },
    });
    expect(calendarColorStyle(readCalendarColors(), "joel@example.com", "primary")).toEqual({ "--calendar-color": "#1f9a8f" });
    expect(calendarColorStyle(readCalendarColors(), "other@example.com", "team")).toBeUndefined();
  });

  it("replaces a calendar's earlier color", () => {
    setCalendarColor("joel@example.com", "primary", "teal");
    setCalendarColor("joel@example.com", "primary", "pink");
    expect(readCalendarColors()).toEqual({ "joel@example.com": { primary: "pink" } });
  });

  it("never reads a stored value outside the palette into a style", () => {
    localStorage.setItem(CALENDAR_COLORS_KEY, JSON.stringify({
      "joel@example.com": {
        primary: "teal",
        injected: "red; background-image: url(https://tracker.example/pixel)",
        hex: "#123456",
        number: 4,
      },
      "list@example.com": ["teal"],
      "text@example.com": "teal",
    }));
    resetCalendarColorsForTests();
    expect(readCalendarColors()).toEqual({ "joel@example.com": { primary: "teal" } });
  });

  it("ignores ids that are not in the palette when saving", () => {
    setCalendarColor("joel@example.com", "primary", "url(x)" as never);
    expect(localStorage.getItem(CALENDAR_COLORS_KEY)).toBeNull();
  });

  it("falls back to no colors when storage is malformed", () => {
    localStorage.setItem(CALENDAR_COLORS_KEY, "{not json");
    expect(readCalendarColors()).toEqual({});
    localStorage.setItem(CALENDAR_COLORS_KEY, "null");
    expect(readCalendarColors()).toEqual({});
  });

  it("leaves events without a calendar id on the default color", () => {
    setCalendarColor("joel@example.com", "primary", "teal");
    expect(calendarColorStyle(readCalendarColors(), "joel@example.com", undefined)).toBeUndefined();
  });
});
