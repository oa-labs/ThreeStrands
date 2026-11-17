import { afterEach, describe, expect, it, vi } from "vitest";
import { planItemKey } from "./dailyPlan";
import type { DailyPlan, DailyPlanItem, PlanReview, PlanReviewItem } from "./domain";
import { carriedToTomorrow, dayClosed, decisionChanges, decisionEntry, looseEnds, readViewChoice, resolveView, reviewsToday, TODAY_VIEW_KEY, writeViewChoice } from "./eveningView";

const TODAY = "2026-11-17";
const TOMORROW = "2026-11-18";
const ref = (id: string) => ({ version: 1 as const, kind: "task" as const, id });
const item = (id: string, overrides: Partial<DailyPlanItem> = {}): DailyPlanItem => ({
  key: planItemKey(ref(id)), kind: "task", reference: ref(id), title: `Task ${id}`, state: "open", marker: null, taskId: id,
  accountId: null, goal: null, contactId: null, carryCount: 0, start: 0, end: 0, ...overrides,
});
const row = (id: string, overrides: Partial<PlanReviewItem> = {}): PlanReviewItem => ({ ...item(id), plannedOn: TODAY, ...overrides });
const plan = (notToday: DailyPlanItem[] = []): DailyPlan => ({ date: TODAY, noteId: `daily:${TODAY}`, revision: "r1", items: [], notToday });
const review = (overrides: Partial<PlanReview> = {}): PlanReview => ({ date: TOMORROW, reviewed: TODAY, done: [], alsoDone: [], meetings: null, didntGetTo: [], ...overrides });

afterEach(() => { localStorage.clear(); vi.restoreAllMocks(); });

describe("the remembered view", () => {
  it("lasts for the day it was chosen and no longer", () => {
    expect(readViewChoice(TODAY)).toBeNull();
    writeViewChoice(TODAY, "evening");
    expect(readViewChoice(TODAY)).toBe("evening");
    expect(readViewChoice(TOMORROW)).toBeNull();
    writeViewChoice(TODAY, "day");
    expect(readViewChoice(TODAY)).toBe("day");
  });

  it("ignores anything unreadable, and survives storage that throws", () => {
    for (const bad of ["garbage", "{}", JSON.stringify({ day: TODAY, view: "night" }), JSON.stringify({ day: 1, view: "day" })]) {
      localStorage.setItem(TODAY_VIEW_KEY, bad);
      expect(readViewChoice(TODAY)).toBeNull();
    }
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => { throw new Error("blocked"); });
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => { throw new Error("blocked"); });
    expect(readViewChoice(TODAY)).toBeNull();
    expect(() => writeViewChoice(TODAY, "evening")).not.toThrow();
  });

  it("prefers the choice over the clock in both directions", () => {
    expect(resolveView(null, false)).toBe("day");
    expect(resolveView(null, true)).toBe("evening");
    expect(resolveView("evening", false)).toBe("evening");
    expect(resolveView("day", true)).toBe("day");
  });
});

describe("loose ends and decisions", () => {
  it("knows when the review describes today's own plan", () => {
    expect(reviewsToday(review(), TODAY)).toBe(true);
    expect(reviewsToday(review({ reviewed: "2026-11-16" }), TODAY)).toBe(false);
    expect(reviewsToday(review({ reviewed: null }), TODAY)).toBe(false);
    expect(reviewsToday(null, TODAY)).toBe(false);
  });

  it("leaves out what today's note already decided, keyed as natively, and nothing without a review", () => {
    const rows = [row("a"), row("b"), row("c")];
    expect(looseEnds(null, plan())).toEqual([]);
    expect(looseEnds(review({ didntGetTo: rows }), null).map((entry) => entry.title)).toEqual(["Task a", "Task b", "Task c"]);
    expect(looseEnds(review({ didntGetTo: rows }), plan([item("b", { marker: { kind: "dropped" } })])).map((entry) => entry.title)).toEqual(["Task a", "Task c"]);
  });

  it("lists today's carries to tomorrow, and only those", () => {
    const carried = item("a", { marker: { kind: "deferred", date: TOMORROW } });
    const later = item("b", { marker: { kind: "deferred", date: "2026-11-23" } });
    const dropped = item("c", { marker: { kind: "dropped" } });
    expect(carriedToTomorrow(plan([carried, later, dropped]), TOMORROW)).toEqual([carried]);
    expect(carriedToTomorrow(null, TOMORROW)).toEqual([]);
  });

  it("turns each decision into the Not today line it writes, from tomorrow on", () => {
    const memo = row("memo", { title: "Draft the memo" });
    expect(decisionEntry(memo, "carry", TOMORROW)).toEqual({ reference: ref("memo"), label: "Draft the memo", decision: { kind: "deferred", date: TOMORROW } });
    expect(decisionEntry(memo, "drop", TOMORROW)).toEqual({ reference: ref("memo"), label: "Draft the memo", decision: { kind: "dropped" } });
    expect(decisionEntry(memo, "defer", TOMORROW, "2026-11-23")?.decision).toEqual({ kind: "deferred", date: "2026-11-23" });
    // Exactly tomorrow is the limit; today and earlier, or no date, write nothing.
    expect(decisionEntry(memo, "defer", TOMORROW, TOMORROW)?.decision).toEqual({ kind: "deferred", date: TOMORROW });
    expect(decisionEntry(memo, "defer", TOMORROW, TODAY)).toBeNull();
    expect(decisionEntry(memo, "defer", TOMORROW)).toBeNull();
    expect(decisionEntry(row("gone", { state: "unavailable" }), "carry", TOMORROW)).toBeNull();
    expect(decisionEntry(row("gone", { state: "unavailable" }), "drop", TOMORROW)).not.toBeNull();
    expect(decisionEntry(row("text", { reference: null }), "drop", TOMORROW)).toBeNull();
  });

  it("saves one item-level change under a new operation id each time", () => {
    const entry = decisionEntry(row("a"), "drop", TOMORROW)!;
    const first = decisionChanges(entry);
    expect(first.changes).toEqual({ items: [], remove: [], notToday: [entry] });
    expect(decisionChanges(entry).operationId).not.toBe(first.operationId);
  });
});

describe("dayClosed", () => {
  it("needs every count known and zero", () => {
    expect(dayClosed(0, 0)).toBe(true);
    expect(dayClosed(1, 0)).toBe(false);
    expect(dayClosed(0, 2)).toBe(false);
    expect(dayClosed(null, 0)).toBe(false);
    expect(dayClosed(0, null)).toBe(false);
    expect(dayClosed(null, null)).toBe(false);
  });
});
