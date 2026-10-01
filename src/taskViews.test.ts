import { describe, expect, it } from "vitest";
import type { ThreadTask } from "./domain";
import { adjacentTaskStatus, compareTasksForDisplay, taskBoardColumn, taskBoardColumnStatus, taskMatchesView, taskViewForAll } from "./taskViews";

const now = new Date(2026, 8, 25, 12, 0);
const task = (overrides: Partial<ThreadTask> = {}): ThreadTask => ({
  id: "task-1", accountId: "me@example.com", threadId: null, subjectSnapshot: null,
  title: "Plan launch", kind: "action", dueKind: "none", dueValue: null,
  status: "open", createdAt: "2026-09-20T10:00:00Z", updatedAt: "2026-09-20T10:00:00Z",
  ...overrides,
});

describe("task workspace views", () => {
  it("keeps date-only tasks due today out of Overdue until the next day", () => {
    expect(taskViewForAll(task({ dueKind: "date", dueValue: "2026-09-24" }), now)).toBe("Overdue");
    expect(taskViewForAll(task({ dueKind: "date", dueValue: "2026-09-25" }), now)).toBe("Today");
    expect(taskViewForAll(task({ dueKind: "date", dueValue: "2026-09-26" }), now)).toBe("Upcoming");
  });

  it("treats a passed due time as Overdue while a later time today remains Today", () => {
    const earlier = new Date(2026, 8, 25, 11, 59).toISOString();
    const later = new Date(2026, 8, 25, 12, 1).toISOString();
    expect(taskViewForAll(task({ dueKind: "datetime", dueValue: earlier }), now)).toBe("Overdue");
    expect(taskViewForAll(task({ dueKind: "datetime", dueValue: later }), now)).toBe("Today");
  });

  it("includes due waiting tasks in both the dated view and Waiting, but only once in All", () => {
    const waiting = task({ kind: "waiting_for", dueKind: "date", dueValue: "2026-09-24" });
    expect(taskViewForAll(waiting, now)).toBe("Overdue");
    expect(taskMatchesView(waiting, "Overdue", now)).toBe(true);
    expect(taskMatchesView(waiting, "Waiting", now)).toBe(true);
    expect(taskViewForAll(task({ kind: "waiting_for" }), now)).toBe("Waiting");
    expect(taskViewForAll(task(), now)).toBe("Anytime");
  });

  it("keeps completed work separate from active views", () => {
    const completed = task({ status: "completed" });
    expect(taskMatchesView(completed, "All", now)).toBe(false);
    expect(taskMatchesView(completed, "Completed", now)).toBe(true);
  });

  it("treats in-progress tasks as active work in every open view", () => {
    const started = task({ status: "in_progress", dueKind: "date", dueValue: "2026-09-25" });
    expect(taskMatchesView(started, "All", now)).toBe(true);
    expect(taskMatchesView(started, "Today", now)).toBe(true);
    expect(taskMatchesView(started, "Completed", now)).toBe(false);
    expect(taskViewForAll(started, now)).toBe("Today");
  });
});

describe("task board columns", () => {
  it("places each status in exactly one column", () => {
    expect(taskBoardColumn("open")).toBe("To Do");
    expect(taskBoardColumn("in_progress")).toBe("In Progress");
    expect(taskBoardColumn("completed")).toBe("Done");
    expect(taskBoardColumn("cancelled")).toBe("Done");
  });

  it("moves one column at a time and stops at the edges", () => {
    expect(adjacentTaskStatus("open", 1)).toBe("in_progress");
    expect(adjacentTaskStatus("in_progress", 1)).toBe("completed");
    expect(adjacentTaskStatus("completed", 1)).toBeNull();
    expect(adjacentTaskStatus("cancelled", -1)).toBe("in_progress");
    expect(adjacentTaskStatus("in_progress", -1)).toBe("open");
    expect(adjacentTaskStatus("open", -1)).toBeNull();
  });

  it("maps each column back to the status a dropped task takes", () => {
    expect(taskBoardColumnStatus("To Do")).toBe("open");
    expect(taskBoardColumnStatus("In Progress")).toBe("in_progress");
    expect(taskBoardColumnStatus("Done")).toBe("completed");
  });
});

describe("task display order", () => {
  const ids = (tasks: ThreadTask[]) => [...tasks].sort(compareTasksForDisplay).map((item) => item.id);

  it("puts the soonest due first so overdue work rises, with undated tasks after in creation order", () => {
    expect(ids([
      task({ id: "undated-new", createdAt: "2026-09-22T10:00:00Z" }),
      task({ id: "later", dueKind: "date", dueValue: "2026-10-02" }),
      task({ id: "undated-old", createdAt: "2026-09-18T10:00:00Z" }),
      task({ id: "overdue", dueKind: "date", dueValue: "2026-09-20" }),
      task({ id: "today-noon", dueKind: "datetime", dueValue: new Date(2026, 8, 25, 12, 0).toISOString() }),
      task({ id: "today", dueKind: "date", dueValue: "2026-09-25" }),
    ])).toEqual(["overdue", "today-noon", "today", "later", "undated-old", "undated-new"]);
  });

  it("orders finished work by most recently completed, after active work", () => {
    expect(ids([
      task({ id: "done-old", status: "completed", completedAt: "2026-09-20T10:00:00Z" }),
      task({ id: "open" }),
      task({ id: "cancelled-new", status: "cancelled", completedAt: null, updatedAt: "2026-09-24T10:00:00Z" }),
      task({ id: "done-new", status: "completed", completedAt: "2026-09-23T10:00:00Z" }),
    ])).toEqual(["open", "cancelled-new", "done-new", "done-old"]);
  });

  it("keeps an unparseable due value with the undated tasks instead of throwing", () => {
    expect(ids([
      task({ id: "broken", dueKind: "datetime", dueValue: "not-a-date", createdAt: "2026-09-19T10:00:00Z" }),
      task({ id: "dated", dueKind: "date", dueValue: "2026-09-30" }),
    ])).toEqual(["dated", "broken"]);
  });
});
