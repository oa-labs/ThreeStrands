import { describe, expect, it } from "vitest";
import type { Goal, ThreadTask } from "./domain";
import { carryForwardRequest, currentGoalGroups, formatPeriod, GOAL_STALE_AFTER_DAYS, goalCountsByOtherAccount, goalIsStale, goalOptionsForTask, goalPeriodMatches, goalProgress, goalsToReview, parentCandidates, periodEncloses, periodEnded, periodFor, shiftPeriod } from "./goals";

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
  it("counts goals on every account but the current one, sorted by account", () => {
    const goals = [
      goal("mine"),
      goal("z1", { accountId: "zed@example.com" }),
      goal("a1", { accountId: "amy@example.com" }),
      goal("z2", { accountId: "zed@example.com" }),
    ];
    expect(goalCountsByOtherAccount(goals, "you@example.com")).toEqual([
      { accountId: "amy@example.com", count: 1 },
      { accountId: "zed@example.com", count: 2 },
    ]);
    expect(goalCountsByOtherAccount([goal("mine")], "you@example.com")).toEqual([]);
  });

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

describe("goal reviews and staleness", () => {
  const now = new Date(2026, 9, 4);
  const days = (count: number) => new Date(now.getTime() - count * 24 * 60 * 60 * 1000).toISOString();

  it("ends a period only once the current period of its horizon has moved past it", () => {
    expect(periodEnded("quarter", "2026-Q3", now)).toBe(true);
    expect(periodEnded("quarter", "2026-Q4", now)).toBe(false);
    expect(periodEnded("half", "2026-H1", now)).toBe(true);
    expect(periodEnded("half", "2026-H2", now)).toBe(false);
    expect(periodEnded("year", "2025", now)).toBe(true);
    expect(periodEnded("year", "2026", now)).toBe(false);
    expect(periodEnded("quarter", "2027-Q1", now)).toBe(false);
  });

  it("flags an active current goal stale only after the threshold with no activity on it or its supporters", () => {
    const year = goal("year", { horizon: "year", period: "2026", createdAt: days(60) });
    const quarter = goal("q4", { parentGoalId: "year", createdAt: days(60) });
    const atLimit = [task("t", { goalId: "q4", updatedAt: days(GOAL_STALE_AFTER_DAYS - 1) })];
    expect(goalIsStale(year, [year, quarter], atLimit, now)).toBe(false);
    const past = [task("t", { goalId: "q4", updatedAt: days(GOAL_STALE_AFTER_DAYS + 1) })];
    expect(goalIsStale(year, [year, quarter], past, now)).toBe(true);
    expect(goalIsStale(quarter, [year, quarter], past, now)).toBe(true);
    // A new goal counts from its creation; closed and ended goals are never stale.
    expect(goalIsStale(goal("new", { createdAt: days(2) }), [], [], now)).toBe(false);
    expect(goalIsStale(goal("done", { status: "achieved", createdAt: days(60) }), [], [], now)).toBe(false);
    expect(goalIsStale(goal("ended", { period: "2026-Q3", createdAt: days(60) }), [], [], now)).toBe(false);
  });

  it("lists ended active goals for review shortest horizon first", () => {
    const review = goalsToReview([
      goal("h1", { horizon: "half", period: "2026-H1" }),
      goal("q3", { period: "2026-Q3" }),
      goal("q2", { period: "2026-Q2" }),
      goal("y", { horizon: "year", period: "2025" }),
      goal("done", { period: "2026-Q3", status: "dropped" }),
      goal("now", { period: "2026-Q4" }),
    ], now);
    expect(review.map((item) => item.id)).toEqual(["q2", "q3", "h1", "y"]);
  });

  it("carries a goal into the current period and drops a supported goal that no longer encloses it", () => {
    const year = goal("year", { horizon: "year", period: "2026" });
    const firstHalf = goal("h1", { horizon: "half", period: "2026-H1" });
    expect(carryForwardRequest(goal("q3", { period: "2026-Q3", parentGoalId: "year" }), [year], now)).toEqual({ period: "2026-Q4" });
    expect(carryForwardRequest(goal("q2", { period: "2026-Q2", parentGoalId: "h1" }), [firstHalf], now)).toEqual({ period: "2026-Q4", parentGoalId: null });
    expect(carryForwardRequest(goal("old", { horizon: "year", period: "2025" }), [], now)).toEqual({ period: "2026" });
  });
});
