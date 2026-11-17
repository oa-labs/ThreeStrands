import { describe, expect, it } from "vitest";
import type { ContactProfile, Goal, ThreadTask } from "../domain";
import { todayThread } from "../test/todayMail";
import type { NoteReference } from "../noteEntities";
import { planItemKey } from "../dailyPlan";
import { createDemoNotes } from "./demoNotes";
import { createDemoPlans, MAX_PLAN_ADDITIONS, MAX_PLAN_HISTORY_TARGETS, PLAN_SOURCE_PAGE } from "./demoPlans";
import type { ScheduleEvent } from "../domain";

const DAY = "2026-11-16";
const taskRef = (id: string): NoteReference => ({ version: 1, kind: "task", id });
const task = (id: string, overrides: Partial<ThreadTask> = {}): ThreadTask => ({ id, accountId: "me@example.com", threadId: null, subjectSnapshot: null, title: `Task ${id}`, kind: "action", dueKind: "none", status: "open", createdAt: "", updatedAt: "", ...overrides });
const goal = { id: "g1", accountId: "me@example.com", title: "Ship Q4", notes: null, horizon: "quarter", period: "2026-Q4", status: "active", parentGoalId: null, createdAt: "", updatedAt: "", closedAt: null } as Goal;

function setup(tasks: ThreadTask[] = [], contacts: ContactProfile[] = [], events: ScheduleEvent[] = []) {
  const notes = createDemoNotes({ threads: () => [], tasks: () => tasks, events: () => [] });
  return { notes, plans: createDemoPlans({ tasks: () => tasks, goals: () => [goal], contacts: () => contacts, notes, events: () => events, now: () => new Date(2026, 10, 16, 12) }) };
}
const href = (id: string) => `threestrands:task/${encodeURIComponent(JSON.stringify(taskRef(id)))}`;

describe("demo daily plans", () => {
  it("adds links and new tasks once, resolves their state, and removes one", async () => {
    const { notes, plans } = setup([task("t1", { goalId: "g1", status: "in_progress" })]);
    const first = await plans.addToDailyPlan(DAY, null, [{ kind: "link", reference: taskRef("t1"), label: "Old" }, { kind: "newTask", title: "Draft memo" }]);
    expect(first.note.body).toMatch(/^## Plan\n\n- \[Old\]\(threestrands:task\/.+\)\n- \( \) Draft memo/);
    expect(first.changed).toHaveLength(2);
    const again = await plans.addToDailyPlan(DAY, first.note.updatedAt, [{ kind: "link", reference: taskRef("t1"), label: "Old" }]);
    expect(again.already).toEqual([planItemKey(taskRef("t1"))]);
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

  it("saves a routine's changes once per operation, with Not today decisions, and undoes them only while unchanged", async () => {
    const { notes, plans } = setup([task("t1"), task("t2"), task("t3")]);
    const changes = {
      items: [{ kind: "add" as const, addition: { kind: "link" as const, reference: taskRef("t1"), label: "One" }, carried: "2026-11-13" }, { kind: "add" as const, addition: { kind: "newTask" as const, title: "Draft memo" } }],
      remove: [],
      notToday: [{ reference: taskRef("t2"), label: "Two", decision: { kind: "deferred" as const, date: "2026-11-20" } }, { reference: taskRef("t3"), label: "Three", decision: { kind: "dropped" as const } }],
    };
    const saved = await plans.saveDailyPlan(DAY, null, "op-1", changes);
    expect(saved.note.body).toContain("<!-- threestrands-plan:carried 2026-11-13 -->\n- ( ) Draft memo");
    expect(saved.note.body).toContain("### Not today");
    expect(saved.added).toHaveLength(2);
    expect(await plans.saveDailyPlan(DAY, null, "op-1", changes)).toEqual(saved);
    await expect(plans.saveDailyPlan(DAY, "stale", "op-2", changes)).rejects.toThrow();
    await expect(plans.saveDailyPlan(DAY, saved.note.updatedAt, "op-3", { items: [], remove: [], notToday: [{ reference: taskRef("t2"), label: "x", decision: { kind: "deferred", date: DAY } }] })).rejects.toThrow();

    const plan = await plans.dailyPlan(DAY);
    const reordered = await plans.saveDailyPlan(DAY, plan.revision, "op-4", { items: [{ kind: "existing", key: plan.items[1].key }, { kind: "existing", key: plan.items[0].key }], remove: [], notToday: [] });
    expect(reordered.note.body.indexOf("Draft memo")).toBeLessThan(reordered.note.body.indexOf("[One]"));
    expect((await plans.undoDailyPlanSave(DAY, "op-4")).body).toBe(saved.note.body);
    // After later writing, Undo refuses rather than overwrite it.
    const now = (await notes.getDailyNote(DAY))!;
    await notes.saveDailyNote(DAY, `${now.body}More\n`, now.updatedAt);
    await expect(plans.undoDailyPlanSave(DAY, "op-1")).rejects.toThrow(/changed/);
  });
});

describe("demo plan history, review, and sources", () => {
  it("keeps a completed starred suggestion settled until a later message arrives, without changing the star", async () => {
    const thread = todayThread("t", { subject: "Proposal", starred: true, labels: [], lastReceivedAt: "2026-11-17T10:00:00Z" });
    const work = task("done", { threadId: "t", status: "completed", sourceMessageId: "first" });
    let latest = "first";
    const notes = createDemoNotes({ threads: () => [thread], tasks: () => [work], events: () => [] });
    const plans = createDemoPlans({ tasks: () => [work], goals: () => [], contacts: () => [], notes, threads: () => [thread], latestMessageId: () => latest });
    expect((await plans.planSources(DAY, "starredMail")).candidates).toEqual([]);
    expect(thread.starred).toBe(true);
    work.status = "open";
    expect((await plans.planSources(DAY, "starredMail")).candidates[0].taskId).toBe(work.id);
    work.status = "cancelled";
    expect((await plans.planSources(DAY, "starredMail")).candidates[0].action.kind).toBe("taskForm");
    work.status = "completed"; latest = "later";
    expect((await plans.planSources(DAY, "starredMail")).candidates[0].action.kind).toBe("taskForm");
    expect(thread.starred).toBe(true);
  });
  it("counts carried days, reviews the last plan, and settles items later plans mention", async () => {
    const tasks = [task("a", { title: "Contract" }), task("b", { title: "Deck" }), task("c", { title: "Done", status: "completed", completedAt: new Date(2026, 10, 15, 10).toISOString() }), task("d", { title: "Also done", status: "completed", completedAt: new Date(2026, 10, 15, 16).toISOString() })];
    const events = [{ id: "standup", accountId: "me@example.com", title: "Standup", allDay: false, start: new Date(2026, 10, 15, 9).toISOString(), end: new Date(2026, 10, 15, 9, 15).toISOString() } as ScheduleEvent];
    const { notes, plans } = setup(tasks, [], events);
    await notes.saveDailyNote("2026-11-13", `## Plan\n\n- [A](${href("a")})\n- [B](${href("b")})\n`, null);
    await notes.saveDailyNote("2026-11-14", `## Plan\n\n- [A](${href("a")}) <!-- threestrands-plan:carried 2026-11-13 -->\n\n### Not today\n\n- [B](${href("b")}) <!-- threestrands-plan:deferred 2026-11-16 -->\n`, null);
    await notes.saveDailyNote("2026-11-15", `## Plan\n\n- [A](${href("a")}) <!-- threestrands-plan:carried 2026-11-14 -->\n- [C](${href("c")})\n`, null);

    const [a] = await plans.planHistory([planItemKey(taskRef("a"))], "2026-11-01", "2026-11-16");
    expect(a).toEqual({ target: planItemKey(taskRef("a")), carryCount: 2, plannedDays: ["2026-11-13", "2026-11-14", "2026-11-15"] });
    expect((await plans.dailyPlan("2026-11-15")).items[0].carryCount).toBe(2);
    await expect(plans.planHistory([], "2026-08-18", "2026-11-16")).rejects.toThrow();
    expect(await plans.planHistory([], "2026-08-19", "2026-11-16")).toEqual([]);
    await expect(plans.planHistory(Array.from({ length: MAX_PLAN_HISTORY_TARGETS + 1 }, (_, n) => `k${n}`), DAY, DAY)).rejects.toThrow();

    const review = await plans.planReview(DAY);
    expect(review.reviewed).toBe("2026-11-15");
    expect(review.done.map((item) => item.title)).toEqual(["Done"]);
    expect(review.alsoDone.map((item) => item.title)).toEqual(["Also done"]);
    expect(review.meetings?.map((event) => event.id)).toEqual(["standup"]);
    expect(review.didntGetTo.map((item) => [item.title, item.plannedOn, item.carryCount])).toEqual([["Contract", "2026-11-15", 2], ["Deck", "2026-11-14", 0]]);
    // Nothing within seven days: nothing to review.
    expect((await plans.planReview("2026-11-23")).reviewed).toBeNull();
  });

  it("includes tasks in today's note while keeping square checklists local and planned identities intact", async () => {
    const { notes, plans } = setup([]);
    await notes.saveDailyNote(DAY, "## Plan\n\n- ( ) Planned task\n\n## Meetings\n\n- ( ) Prepare discussion\n- [ ] Local checklist\n", null);
    const page = await plans.planSources(DAY, "notes");
    expect(page.candidates.map((candidate) => candidate.title)).toEqual(["Planned task", "Prepare discussion"]);
    expect(page.candidates.find((candidate) => candidate.title === "Planned task")?.planned).toBe(true);
    expect(page.candidates.find((candidate) => candidate.title === "Prepare discussion")?.planned).toBe(false);
  });

  it("pages sources, marks planned candidates, and offers suggested work with reasons", async () => {
    const tasks = [
      ...Array.from({ length: PLAN_SOURCE_PAGE }, (_, n) => task(`t${n}`)),
      task("late", { title: "Late", dueKind: "date", dueValue: "2026-11-10" }),
    ];
    const { plans } = setup(tasks);
    const first = await plans.planSources(DAY, "tasks");
    expect(first.candidates).toHaveLength(PLAN_SOURCE_PAGE);
    expect(first.candidates[0]).toMatchObject({ title: "Late", reason: "overdue" });
    expect(first.next).toBe(String(PLAN_SOURCE_PAGE));
    expect((await plans.planSources(DAY, "tasks", null, first.next)).candidates).toHaveLength(1);
    const suggested = await plans.planSources(DAY, "suggested");
    expect(suggested.candidates.map((candidate) => [candidate.title, candidate.reason])).toEqual([["Late", "overdue"]]);
    const add = suggested.candidates[0].action;
    if (add.kind !== "link") throw new Error("expected a link");
    await plans.addToDailyPlan(DAY, null, [add.addition]);
    expect((await plans.planSources(DAY, "suggested")).candidates[0].planned).toBe(true);
    expect((await plans.dailyPlan(DAY)).items[0].key).toBe(suggested.candidates[0].key);
    await expect(plans.planSources(DAY, "tasks", null, "x")).rejects.toThrow();
  });
});
