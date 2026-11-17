import { afterEach, describe, expect, it } from "vitest";
import { planItemKey } from "./dailyPlan";
import type { DailyPlan, DailyPlanItem, PlanReviewItem } from "./domain";
import {
  addItem, addNewTask, carryAll, decide, discardRecovery, hasChoices, isDirty, MAX_RECOVERED_ROUTINES, moveItem, planChanges, planText, previewLines,
  readRecovery, rebase, removeItem, ROUTINE_RECOVERY_DAYS, ROUTINE_RECOVERY_KEY, startRoutine, writeRecovery,
} from "./planRoutine";

const DAY = "2026-11-17";
const ref = (id: string) => ({ version: 1 as const, kind: "task" as const, id });
const item = (id: string, overrides: Partial<DailyPlanItem> = {}): DailyPlanItem => ({
  key: planItemKey(ref(id)), kind: "task", reference: ref(id), title: `Task ${id}`, state: "open", marker: null, taskId: id, accountId: "me@example.com",
  goal: null, contactId: null, carryCount: 0, start: 0, end: 0, ...overrides,
});
const plan = (items: DailyPlanItem[]): DailyPlan => ({ date: DAY, noteId: `daily:${DAY}`, revision: "r1", items, notToday: [] });
const row = (id: string, overrides: Partial<PlanReviewItem> = {}): PlanReviewItem => ({ ...item(id), plannedOn: "2026-11-16", ...overrides });
const link = (id: string) => ({ kind: "link" as const, reference: ref(id), label: `Task ${id}` });
const block = (id: string) => ({ title: `Task ${id}`, sourceTaskId: id, accountId: null });

afterEach(() => window.localStorage.clear());

describe("plan routine", () => {
  it("starts from the plan's items in order and changes nothing until something is chosen", () => {
    const state = startRoutine(DAY, plan([item("a"), item("b")]));
    expect(state.items.map((entry) => entry.title)).toEqual(["Task a", "Task b"]);
    expect(state.edit).toBe(true);
    expect(state.base).toBe("r1");
    expect(isDirty(state)).toBe(false);
    expect(planChanges(state)).toEqual({ items: [{ kind: "existing", key: item("a").key }, { kind: "existing", key: item("b").key }], remove: [], notToday: [] });
    expect(startRoutine(DAY, plan([])).edit).toBe(false);
  });

  it("adds each target once, reorders, and removes a plan item's line rather than dropping it", () => {
    let state = startRoutine(DAY, plan([item("a")]));
    state = addItem(state, { key: planItemKey(ref("b")), title: "Task b", addition: link("b"), carried: null, block: block("b") });
    expect(addItem(state, { key: planItemKey(ref("b")), title: "Task b", addition: link("b"), carried: null, block: block("b") })).toBe(state);
    state = addNewTask(state, "  Draft   memo ");
    expect(addNewTask(state, "  ")).toBe(state);
    state = moveItem(state, "new:1", -1);
    expect(state.items.map((entry) => entry.title)).toEqual(["Task a", "Draft memo", "Task b"]);
    expect(moveItem(state, state.items[0].id, -1)).toBe(state);
    state = removeItem(state, item("a").key);
    expect(planChanges(state)).toEqual({
      items: [{ kind: "add", addition: { kind: "newTask", title: "Draft memo" }, carried: null }, { kind: "add", addition: link("b"), carried: null }],
      remove: [item("a").key],
      notToday: [],
    });
    // Choosing it again puts it back rather than adding a second line.
    state = addItem(state, { key: item("a").key, title: "Task a", addition: link("a"), carried: null, block: block("a") });
    expect(planChanges(state).remove).toEqual([]);
    expect(state.items.at(-1)?.existing).toBe(true);
  });

  it("carries, defers, and drops review rows, with Carry All for the rest and no carrying of unavailable items", () => {
    const rows = [row("a"), row("b"), row("c"), row("gone", { state: "unavailable" })];
    let state = startRoutine(DAY, plan([]));
    state = decide(state, rows[1], { kind: "defer", date: "2026-11-20" });
    state = decide(state, rows[3], { kind: "carry" });
    expect(state.decisions[rows[3].key]).toBeUndefined();
    state = decide(state, rows[3], { kind: "drop" });
    state = carryAll(state, rows);
    expect(state.items.map((entry) => [entry.title, entry.carried])).toEqual([["Task a", "2026-11-16"], ["Task c", "2026-11-16"]]);
    const changes = planChanges(state);
    expect(changes.items).toEqual([{ kind: "add", addition: link("a"), carried: "2026-11-16" }, { kind: "add", addition: link("c"), carried: "2026-11-16" }]);
    expect(changes.notToday).toEqual([
      { reference: ref("b"), label: "Task b", decision: { kind: "deferred", date: "2026-11-20" } },
      { reference: ref("gone"), label: "Task gone", decision: { kind: "dropped" } },
    ]);
    // Changing a carry to a drop takes the item back out; undeciding forgets it.
    state = decide(state, rows[0], { kind: "drop" });
    expect(state.items.map((entry) => entry.title)).toEqual(["Task c"]);
    state = decide(state, rows[0], null);
    expect(state.decisions[rows[0].key]).toBeUndefined();
    // Removing a carried item undoes its carry.
    state = removeItem(state, rows[2].key);
    expect(state.decisions[rows[2].key]).toBeUndefined();
  });

  it("previews exactly the lines a save adds and copies a plain-text plan", () => {
    let state = startRoutine(DAY, plan([item("a")]));
    state = decide(state, row("b"), { kind: "carry" });
    state = decide(state, row("c"), { kind: "drop" });
    state = addNewTask(state, "Draft memo");
    const lines = previewLines(state);
    expect(lines.plan).toEqual([
      expect.stringMatching(/^- \[Task b\]\(threestrands:task\/.+\) <!-- threestrands-plan:carried 2026-11-16 -->$/),
      "- ( ) Draft memo",
    ]);
    expect(lines.notToday).toEqual([expect.stringMatching(/^- \[Task c\]\(.+\) <!-- threestrands-plan:dropped -->$/)]);
    const text = planText(state);
    expect(text).toMatch(/^Plan for .+\n\n1\. Task a\n2\. Task b\n3\. Draft memo\n\nNot today\n- Task c \(dropped\)\n$/);
    expect(text).not.toContain("threestrands:");
  });

  it("re-bases restored choices on the plan as it is now, marking plan items that are gone", () => {
    let state = startRoutine(DAY, plan([item("a"), item("b")]));
    state = removeItem(state, item("b").key);
    const now = rebase(state, { ...plan([item("b")]), revision: "r2" });
    expect(now.base).toBe("r2");
    expect(now.items[0].missing).toBe(true);
    expect(planChanges(now).items).toEqual([]);
    expect(now.removed.map((entry) => entry.key)).toEqual([item("b").key]);
  });

  it("keeps at most two unfinished routines for a week, one per date, without note or task bodies", () => {
    const at = new Date(2026, 10, 17, 9);
    const make = (date: string, savedAt: Date) => ({ ...addNewTask(startRoutine(date, null), "Draft memo"), savedAt: savedAt.toISOString() });
    writeRecovery(make("2026-11-15", new Date(2026, 10, 15)), at);
    writeRecovery(make("2026-11-16", new Date(2026, 10, 16)), at);
    writeRecovery(make(DAY, at), at);
    expect(MAX_RECOVERED_ROUTINES).toBe(2);
    expect(readRecovery("2026-11-15", at)).toBeNull();
    expect(readRecovery(DAY, at)?.items[0].title).toBe("Draft memo");
    expect(hasChoices(readRecovery(DAY, at)!)).toBe(true);
    // Older than the window: forgotten.
    expect(readRecovery("2026-11-16", new Date(at.getTime() + (ROUTINE_RECOVERY_DAYS + 1) * 86_400_000))).toBeNull();
    discardRecovery(DAY, at);
    expect(readRecovery(DAY, at)).toBeNull();
    const stored = window.localStorage.getItem(ROUTINE_RECOVERY_KEY) ?? "";
    expect(stored).not.toContain("body");
    // Unreadable records are ignored.
    window.localStorage.setItem(ROUTINE_RECOVERY_KEY, "{not json");
    expect(readRecovery(DAY, at)).toBeNull();
  });
});
