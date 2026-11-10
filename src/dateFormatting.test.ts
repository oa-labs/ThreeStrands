import { describe, expect, it } from "vitest";
import { formatCompactDate, formatDateTime, formatDiagnosticDateTime, formatFullDate, formatMonthYear } from "./dateFormatting";
import { formatHistoryDate } from "./contactContext";
import { formatMailTimestamp } from "./threadPresentation";

const full = (date: Date) => new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric", year: "numeric" }).format(date);

describe("date display policy", () => {
  it("uses the same exact compact date in mail and history, retaining old and future years", () => {
    const now = new Date(2026, 8, 18);
    for (const year of [2025, 2026, 2027]) {
      const date = new Date(year, 8, 5, 15, 4);
      const expected = year === 2026
        ? new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric" }).format(date)
        : full(date);
      expect(formatCompactDate(date, now)).toBe(expected);
      expect(formatMailTimestamp(date.toISOString(), now)).toBe(expected);
      expect(formatHistoryDate(date.toISOString(), now)).toBe(expected);
    }
  });

  it("uses local calendar years around midnight at New Year", () => {
    const date = new Date(2025, 11, 31, 23, 30);
    expect(formatCompactDate(date, new Date(2026, 0, 1, 0, 30))).toBe(full(date));
    expect(formatCompactDate(date, new Date(2025, 11, 31, 23, 59))).toBe(
      new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric" }).format(date),
    );
  });

  it("keeps filter years explicit and reserves month precision for aggregates", () => {
    const date = new Date(2026, 8, 5);
    expect(formatFullDate(date)).toBe(full(date));
    expect(formatMonthYear(date)).toBe(new Intl.DateTimeFormat(undefined, { month: "short", year: "numeric" }).format(date));
  });

  it("retains seconds only in operational timestamps", () => {
    const first = new Date(2026, 8, 5, 15, 4, 1);
    const second = new Date(2026, 8, 5, 15, 4, 59);
    expect(formatDateTime(first)).toBe(formatDateTime(second));
    expect(formatDateTime(first)).toBe(new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" }).format(first));
    expect(formatDiagnosticDateTime(first)).not.toBe(formatDiagnosticDateTime(second));
  });
});
