import { describe, expect, it } from "vitest";
import type { Goal, ThreadTask } from "./domain";
import { currentGoalGroups, formatPeriod, goalOptionsForTask, goalPeriodMatches, goalProgress, parentCandidates, periodEncloses, periodFor, shiftPeriod } from "./goals";

function goal(id: string, overrides: Partial<Goal> = {}): Goal {
  return {
    id, accountId: "you@example.com", title: id, notes: null, horizon: "quarter", period: "2026-Q4",
    status: "active", parentGoalId: null, createdAt: "2026-10-01T00:00:00Z", updatedAt: "2026-10-01T00:00:00Z", closedAt: null,
    ...overrides,
  };
}

function task(id: string, overrides: Partial<ThreadTask> = {}): ThreadTask {
  return {
    id, accountId: "you@example.com", threadId: null, subjectSnapshot: null, title: id, kind: "action", dueKind: "none",
    status: "open", createdAt: "2026-10-01T00:00:00Z", updatedAt: "2026-10-01T00:00:00Z", ...overrides,
  };
}

describe("goal periods", () => {
  it("names the period of each horizon that contains a date, at both edges of each period", () => {
    expect(periodFor("year", new Date(2026, 0, 1))).toBe("2026");
    expect(periodFor("half", new Date(2026, 5, 30))).toBe("2026-H1");
    expect(periodFor("half", new Date(2026, 6, 1))).toBe("2026-H2");
    expect(periodFor("quarter", new Date(2026, 2, 31))).toBe("2026-Q1");
    expect(periodFor("quarter", new Date(2026, 3, 1))).toBe("2026-Q2");
    expect(periodFor("quarter", new Date(2026, 11, 31))).toBe("2026-Q4");
  });

  it("matches periods to horizons like the sync contract", () => {
    expect(goalPeriodMatches("year", "2026")).toBe(true);
    expect(goalPeriodMatches("half", "2026-H2")).toBe(true);
    expect(goalPeriodMatches("quarter", "2026-Q4")).toBe(true);
    for (const [horizon, period] of [["year", "2026-Q1"], ["half", "2026-H3"], ["quarter", "2026-Q5"], ["quarter", "2026-q4"], ["quarter", "2026"]] as const) {
      expect(goalPeriodMatches(horizon, period)).toBe(false);
    }
  });

  it("formats and steps periods across year boundaries", () => {
    expect(formatPeriod("2026")).toBe("2026");
    expect(formatPeriod("2026-Q4")).toBe("Q4 2026");
    expect(shiftPeriod("quarter", "2026-Q4", 1)).toBe("2027-Q1");
    expect(shiftPeriod("quarter", "2026-Q1", -1)).toBe("2025-Q4");
    expect(shiftPeriod("half", "2026-H2", 1)).toBe("2027-H1");
    expect(shiftPeriod("half", "2026-H1", -1)).toBe("2025-H2");
    expect(shiftPeriod("year", "2026", -1)).toBe("2025");
  });

  it("encloses a shorter period only inside its own span", () => {
    expect(periodEncloses("2026", "2026-Q1")).toBe(true);
    expect(periodEncloses("2026-H1", "2026-Q2")).toBe(true);
    expect(periodEncloses("2026-H2", "2026-Q3")).toBe(true);
    expect(periodEncloses("2026-H2", "2026-Q2")).toBe(false);
    expect(periodEncloses("2026", "2027-Q1")).toBe(false);
    expect(periodEncloses("2026", "2026")).toBe(false);
  });
});

describe("goal links", () => {
  const year = goal("year", { horizon: "year", period: "2026" });
  const half = goal("half", { horizon: "half", period: "2026-H2" });
  const firstHalf = goal("first-half", { horizon: "half", period: "2026-H1" });
  const other = goal("other", { horizon: "year", period: "2026", accountId: "other@example.com" });
  const closed = goal("closed", { status: "achieved" });

  it("offers longer goals in the same account whose period encloses the goal", () => {
    const candidates = parentCandidates([year, half, firstHalf, other, goal("q4")], goal("q4"));
    expect(candidates.map((candidate) => candidate.id)).toEqual(["year", "half"]);
    expect(parentCandidates([year, half], year)).toEqual([]);
  });

  it("offers a task its account's active goals and keeps its current goal even once closed", () => {
    expect(goalOptionsForTask([year, other, closed], task("t")).map((option) => option.id)).toEqual(["year"]);
    expect(goalOptionsForTask([year, other, closed], task("t", { goalId: "closed" })).map((option) => option.id)).toEqual(["year", "closed"]);
  });

  it("counts linked open and finished tasks rather than a percentage", () => {
    const tasks = [
      task("a", { goalId: "year" }), task("b", { goalId: "year", status: "in_progress" }),
      task("c", { goalId: "year", status: "completed" }), task("d", { goalId: "year", status: "cancelled" }), task("e"),
    ];
    expect(goalProgress(year, tasks)).toEqual({ open: 2, done: 2 });
    expect(goalProgress(half, tasks)).toEqual({ open: 0, done: 0 });
  });

  it("groups the current quarter, half, and year", () => {
    const groups = currentGoalGroups([year, half, firstHalf, goal("q4"), goal("q3", { period: "2026-Q3" })], new Date(2026, 9, 4));
    expect(groups.map((group) => [group.horizon, group.period, group.goals.map((item) => item.id)])).toEqual([
      ["quarter", "2026-Q4", ["q4"]],
      ["half", "2026-H2", ["half"]],
      ["year", "2026", ["year"]],
    ]);
  });
});
