import { describe, expect, it } from "vitest";
import { describeActivity, estimateCadence, organizationDomain } from "./contactContext";
import type { ContactActivity } from "./domain";

const local = (year: number, month: number, day: number) => new Date(year, month - 1, day, 15).toISOString();
const monthlyReports = Array.from({ length: 12 }, (_, index) => local(2026, index + 1, [3, 6, 2, 7, 4, 3, 2, 10, 9, 2, 5, 2][index]));
const activity = (overrides: Partial<ContactActivity> = {}): ContactActivity => ({
  sentCount: 6, receivedCount: 25, threadCount: 1, firstAt: local(2024, 10, 4), lastSentAt: local(2026, 3, 4),
  recentReceivedAt: monthlyReports.slice(0, 10).reverse(), ...overrides,
});

describe("estimateCadence", () => {
  const now = new Date(2026, 9, 3);

  it("recognizes a monthly sender who writes early in the month", () => {
    expect(estimateCadence(monthlyReports.slice(0, 10), now)).toEqual({ label: "monthly", monthPart: "early" });
  });

  it("treats a same-day back-and-forth as one arrival", () => {
    const withBurst = [...monthlyReports.slice(0, 10), new Date(new Date(monthlyReports[9]).getTime() + 3_600_000).toISOString()];
    expect(estimateCadence(withBurst, now)?.label).toBe("monthly");
  });

  it("recognizes weekly and quarterly habits without a month position", () => {
    const weekly = Array.from({ length: 6 }, (_, index) => local(2026, 9, 1 + index * 7));
    expect(estimateCadence(weekly, new Date(2026, 9, 3))).toEqual({ label: "weekly", monthPart: null });
    const quarterly = [local(2025, 7, 15), local(2025, 10, 14), local(2026, 1, 15), local(2026, 4, 16), local(2026, 7, 15)];
    expect(estimateCadence(quarterly, new Date(2026, 8, 1))).toEqual({ label: "quarterly", monthPart: null });
  });

  it("claims no cadence for irregular, sparse, lapsed, or in-between spacing", () => {
    expect(estimateCadence([local(2026, 1, 2), local(2026, 1, 20), local(2026, 4, 1), local(2026, 4, 9), local(2026, 9, 1)], now)).toBeNull();
    expect(estimateCadence(monthlyReports.slice(7, 10), now)).toBeNull();
    expect(estimateCadence(monthlyReports.slice(0, 6), now)).toBeNull();
    expect(estimateCadence(Array.from({ length: 6 }, (_, index) => local(2026, 1, 1 + index * 20)), new Date(2026, 3, 20))).toBeNull();
    expect(estimateCadence(["not a date", "", local(2026, 9, 1)], now)).toBeNull();
  });
});

describe("describeActivity", () => {
  const now = new Date(2026, 9, 3);

  it("summarizes volume, cadence, and the user's latest message", () => {
    expect(describeActivity(activity(), now)).toEqual(["31 emails since Oct 2024", "Monthly, usually early in the month", "You last wrote Mar 4"]);
  });

  it("uses month and year for an older latest message and omits a missing cadence", () => {
    expect(describeActivity(activity({ lastSentAt: local(2025, 6, 10), recentReceivedAt: [] }), now)).toEqual(["31 emails since Oct 2024", "You last wrote Jun 2025"]);
  });

  it("describes a single email by its date and says nothing without history", () => {
    expect(describeActivity(activity({ sentCount: 0, receivedCount: 1, lastSentAt: null, recentReceivedAt: [local(2026, 10, 2)], firstAt: local(2026, 10, 2) }), now)).toEqual(["1 email · Oct 2, 2026"]);
    expect(describeActivity(activity({ sentCount: 0, receivedCount: 0, firstAt: null }), now)).toEqual([]);
  });
});

describe("organizationDomain", () => {
  it("returns the domain for organization addresses in any case", () => {
    expect(organizationDomain("Dan@Wealth.Example.com", ["you@example.org"])).toBe("wealth.example.com");
  });

  it("skips personal mailbox providers, the user's own domains, and malformed addresses", () => {
    expect(organizationDomain("friend@gmail.com", [])).toBeNull();
    expect(organizationDomain("friend@icloud.com", [])).toBeNull();
    expect(organizationDomain("coworker@example.org", ["You@Example.org"])).toBeNull();
    expect(organizationDomain("nobody@localhost", [])).toBeNull();
    expect(organizationDomain("@example.com", [])).toBeNull();
  });
});
