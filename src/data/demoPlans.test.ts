import { describe, expect, it } from "vitest";
import type { ContactProfile, Goal, ThreadTask } from "../domain";
import { noteReferenceKey, type NoteReference } from "../noteEntities";
import { createDemoNotes } from "./demoNotes";
import { createDemoPlans, MAX_PLAN_ADDITIONS } from "./demoPlans";

const DAY = "2026-11-16";
const taskRef = (id: string): NoteReference => ({ version: 1, kind: "task", id });
const task = (id: string, overrides: Partial<ThreadTask> = {}): ThreadTask => ({ id, accountId: "me@example.com", threadId: null, subjectSnapshot: null, title: `Task ${id}`, kind: "action", dueKind: "none", status: "open", createdAt: "", updatedAt: "", ...overrides });
const goal = { id: "g1", accountId: "me@example.com", title: "Ship Q4", notes: null, horizon: "quarter", period: "2026-Q4", status: "active", parentGoalId: null, createdAt: "", updatedAt: "", closedAt: null } as Goal;

function setup(tasks: ThreadTask[] = [], contacts: ContactProfile[] = []) {
  const notes = createDemoNotes({ threads: () => [], tasks: () => tasks, events: () => [] });
  return { notes, plans: createDemoPlans({ tasks: () => tasks, goals: () => [goal], contacts: () => contacts, notes }) };
}

describe("demo daily plans", () => {
  it("adds links and new tasks once, resolves their state, and removes one", async () => {
    const { notes, plans } = setup([task("t1", { goalId: "g1", status: "in_progress" })]);
    const first = await plans.addToDailyPlan(DAY, null, [{ kind: "link", reference: taskRef("t1"), label: "Old" }, { kind: "newTask", title: "Draft memo" }]);
    expect(first.note.body).toMatch(/^## Plan\n\n- \[Old\]\(threestrands:task\/.+\)\n- \( \) Draft memo/);
    expect(first.changed).toHaveLength(2);
    const again = await plans.addToDailyPlan(DAY, first.note.updatedAt, [{ kind: "link", reference: taskRef("t1"), label: "Old" }]);
    expect(again.already).toEqual([noteReferenceKey(taskRef("t1"))]);
    expect(again.note.body).toBe(first.note.body);

    const plan = await plans.dailyPlan(DAY);
    expect(plan.items.map((item) => [item.kind, item.title, item.state])).toEqual([["task", "Task t1", "inProgress"], ["newTask", "Draft memo", "open"]]);
    expect(plan.items[0].goal).toEqual({ id: "g1", title: "Ship Q4" });
    expect(plan.items[1].taskId).toMatch(/^note:daily:2026-11-16:/);

    const removed = await plans.removeFromDailyPlan(DAY, plan.revision, plan.items[0].key);
    expect(removed.note.body).not.toContain("Old");
    expect((await notes.getDailyNote(DAY))?.body).toContain("Draft memo");
  });

  it("refuses stale versions, unknown tasks, and too many items", async () => {
    const { plans } = setup([task("t1")]);
    await expect(plans.addToDailyPlan(DAY, "stale", [{ kind: "newTask", title: "x" }])).rejects.toThrow();
    await expect(plans.addToDailyPlan(DAY, null, [{ kind: "link", reference: taskRef("missing"), label: "x" }])).rejects.toThrow();
    await expect(plans.addToDailyPlan(DAY, null, [])).rejects.toThrow();
    await expect(plans.addToDailyPlan(DAY, null, Array.from({ length: MAX_PLAN_ADDITIONS + 1 }, (_, n) => ({ kind: "newTask" as const, title: `t${n}` })))).rejects.toThrow();
    expect((await plans.dailyPlan(DAY)).revision).toBeNull();
  });
});
