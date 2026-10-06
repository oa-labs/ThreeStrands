import { describe, expect, it } from "vitest";
import {
  MAX_KEEP_IN_TOUCH_DAYS, describeBirthday, describeDue, formatBirthday, frequencyLabel, isKeepInTouchDue, isSnoozeActive,
  keepInTouchStatus, lastTouchAt, nextBirthday, parseIntervalDays, snoozeUntilDate, snoozeUntilDays,
} from "./keepInTouch";

const local = (year: number, month: number, day: number, hour = 12) => new Date(year, month - 1, day, hour);
const now = local(2026, 10, 5, 9);
const noKit = { intervalDays: null, startedAt: null, snoozedUntil: null, snoozedAt: null, lastTouchAt: null };

describe("keep-in-touch status", () => {
  it("groups by the local calendar day the reminder falls on", () => {
    expect(keepInTouchStatus(null, now)).toBeNull();
    expect(keepInTouchStatus("not a date", now)).toBeNull();
    expect(keepInTouchStatus(local(2026, 10, 4, 23).toISOString(), now)).toBe("overdue");
    // Earlier today is due today, not overdue.
    expect(keepInTouchStatus(local(2026, 10, 5, 0).toISOString(), now)).toBe("today");
    expect(keepInTouchStatus(local(2026, 10, 5, 23).toISOString(), now)).toBe("today");
    expect(keepInTouchStatus(local(2026, 10, 6, 0).toISOString(), now)).toBe("week");
    expect(keepInTouchStatus(local(2026, 10, 12).toISOString(), now)).toBe("week");
    expect(keepInTouchStatus(local(2026, 10, 13).toISOString(), now)).toBe("later");
  });

  it("counts only overdue and due-today reminders as due", () => {
    expect(isKeepInTouchDue({ keepInTouchDueAt: local(2026, 10, 1).toISOString() }, now)).toBe(true);
    expect(isKeepInTouchDue({ keepInTouchDueAt: local(2026, 10, 5, 20).toISOString() }, now)).toBe(true);
    expect(isKeepInTouchDue({ keepInTouchDueAt: local(2026, 10, 6).toISOString() }, now)).toBe(false);
    expect(isKeepInTouchDue({ keepInTouchDueAt: null }, now)).toBe(false);
  });

  it("describes the due date in the short date form", () => {
    expect(describeDue(local(2026, 10, 1).toISOString(), now)).toBe("Overdue since Oct 1");
    expect(describeDue(local(2026, 10, 5, 18).toISOString(), now)).toBe("Due today");
    expect(describeDue(local(2026, 10, 12).toISOString(), now)).toBe("Due Oct 12");
    expect(describeDue(local(2027, 1, 3).toISOString(), now)).toBe("Due Jan 3, 2027");
  });
});

describe("keep-in-touch frequencies", () => {
  it("labels presets by name and other intervals by day count", () => {
    expect(frequencyLabel(14)).toBe("Every 2 Weeks");
    expect(frequencyLabel(91)).toBe("Quarterly");
    expect(frequencyLabel(1)).toBe("Every Day");
    expect(frequencyLabel(45)).toBe("Every 45 Days");
  });

  it("accepts whole days from 1 through the maximum only", () => {
    expect(parseIntervalDays("0")).toBeNull();
    expect(parseIntervalDays("1")).toBe(1);
    expect(parseIntervalDays(String(MAX_KEEP_IN_TOUCH_DAYS - 1))).toBe(MAX_KEEP_IN_TOUCH_DAYS - 1);
    expect(parseIntervalDays(String(MAX_KEEP_IN_TOUCH_DAYS))).toBe(MAX_KEEP_IN_TOUCH_DAYS);
    expect(parseIntervalDays(String(MAX_KEEP_IN_TOUCH_DAYS + 1))).toBeNull();
    for (const invalid of ["", "-3", "2.5", "1e2", "ten"]) expect(parseIntervalDays(invalid)).toBeNull();
  });
});

describe("keep-in-touch snoozes", () => {
  it("snoozes to local midnight a number of days ahead", () => {
    expect(snoozeUntilDays(7, now)).toBe(local(2026, 10, 12, 0).toISOString());
  });

  it("accepts a picked date only when it is after today", () => {
    expect(snoozeUntilDate("2026-10-06", now)).toBe(local(2026, 10, 6, 0).toISOString());
    expect(snoozeUntilDate("2026-10-05", now)).toBeNull();
    expect(snoozeUntilDate("2026-02-30", now)).toBeNull();
    expect(snoozeUntilDate("10/06/2026", now)).toBeNull();
  });

  it("reports a snooze as active only while it sets the due date", () => {
    const until = local(2026, 10, 20, 0).toISOString();
    const snoozed = { ...noKit, intervalDays: 7, snoozedUntil: until, snoozedAt: local(2026, 10, 1).toISOString() };
    expect(isSnoozeActive({ keepInTouch: snoozed, keepInTouchDueAt: until })).toBe(true);
    // A newer touch superseded it; the backend derived a different date.
    expect(isSnoozeActive({ keepInTouch: snoozed, keepInTouchDueAt: local(2026, 10, 9).toISOString() })).toBe(false);
  });

  it("takes the newest of mail and a logged touch as the last contact", () => {
    expect(lastTouchAt({ lastInteractedAt: null, keepInTouch: noKit })).toBeNull();
    expect(lastTouchAt({ lastInteractedAt: "2026-09-01T00:00:00Z", keepInTouch: { ...noKit, lastTouchAt: "2026-09-20T00:00:00Z" } })).toBe("2026-09-20T00:00:00Z");
    expect(lastTouchAt({ lastInteractedAt: "2026-09-25T00:00:00Z", keepInTouch: { ...noKit, lastTouchAt: "2026-09-20T00:00:00Z" } })).toBe("2026-09-25T00:00:00Z");
  });
});

describe("birthdays", () => {
  it("finds the next occurrence, counting today", () => {
    expect(nextBirthday("10-05", now)).toMatchObject({ daysAway: 0, turning: null });
    expect(nextBirthday("10-04", now)?.date).toEqual(local(2027, 10, 4, 0));
    expect(nextBirthday("1990-10-19", now)).toMatchObject({ daysAway: 14, turning: 36 });
    expect(nextBirthday("Oct 5", now)).toBeNull();
  });

  it("falls on February 28 outside leap years", () => {
    expect(nextBirthday("02-29", now)?.date).toEqual(local(2027, 2, 28, 0));
    expect(nextBirthday("02-29", local(2027, 3, 1))?.date).toEqual(local(2028, 2, 29, 0));
  });

  it("formats birthdays with and without a year", () => {
    expect(formatBirthday("12-09")).toBe("Dec 9");
    expect(formatBirthday("1984-12-09")).toBe("Dec 9, 1984");
    expect(describeBirthday("10-05", now)).toBe("Today");
    expect(describeBirthday("2000-10-06", now)).toBe("Tomorrow · turns 26");
    expect(describeBirthday("10-12", now)).toBe("Oct 12");
  });
});
