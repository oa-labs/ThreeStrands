import { describe, expect, it } from "vitest";
import type { CalendarAccount, CalendarOption, ScheduleEvent, ThreadTask } from "./domain";
import { bookedBlock, bookedSource, bookedTaskFor, defaultEventCalendar, openTasksFor, primaryCalendarsFirst, TASK_BLOCK_MINUTES, taskBlockRequest, writableCalendars } from "./taskBooking";
import { noteTaskAsTask } from "./noteEntities";

function calendar(id: string, accountId: string, overrides: Partial<CalendarOption> = {}): CalendarOption {
  return { id, accountId, name: id, primary: false, selected: true, writable: true, ...overrides };
}

function task(id: string, overrides: Partial<ThreadTask> = {}): ThreadTask {
  return {
    id, accountId: "joel@example.com", threadId: null, subjectSnapshot: null, title: id, kind: "action",
    dueKind: "none", status: "open", createdAt: "2026-09-01T00:00:00Z", updatedAt: "2026-09-01T00:00:00Z", ...overrides,
  };
}

function block(id: string, start: string, end: string, sourceTaskId: string | null = "proposal"): ScheduleEvent {
  return { id, accountId: "joel@example.com", title: id, start, end, allDay: false, sourceTaskId };
}

function noteTask(title: string, overrides: { checked?: boolean; markerId?: string } = {}): ThreadTask {
  const markerId = overrides.markerId ?? "m1";
  return noteTaskAsTask({ id: `note:daily:2026-11-16:${markerId}`, markerId, noteId: "daily:2026-11-16", noteTitle: "Monday", title, checked: overrides.checked ?? false, updatedAt: "2026-11-16T09:00:00Z", completedAt: null, conflicted: false });
}

describe("task booking", () => {
  it("only offers writable calendars on connected accounts", () => {
    const accounts: CalendarAccount[] = [
      { email: "joel@example.com", connectedAt: "2026-01-01T00:00:00Z", status: "connected" },
      { email: "old@example.com", connectedAt: "2026-01-01T00:00:00Z", status: "needs_reauth" },
    ];
    const calendars = [calendar("primary", "joel@example.com"), calendar("holidays", "joel@example.com", { writable: false }), calendar("old", "old@example.com")];
    expect(writableCalendars(accounts, calendars).map((option) => option.id)).toEqual(["primary"]);
  });

  it("prefers the task's account, then a visible primary calendar, falling back to any writable one", () => {
    const writable = [
      calendar("team", "joel@example.com"),
      calendar("joel", "joel@example.com", { primary: true }),
      calendar("work-team", "work@example.com", { selected: false }),
      calendar("work", "work@example.com", { primary: true, selected: false }),
    ];
    expect(defaultEventCalendar(writable)?.id).toBe("joel");
    expect(defaultEventCalendar(writable, "work@example.com")?.id).toBe("work");
    expect(defaultEventCalendar([writable[2]!, writable[0]!], "work@example.com")?.id).toBe("work-team");
    // An account without a writable calendar books on the usual default.
    expect(defaultEventCalendar(writable, "imap:home")?.id).toBe("joel");
    expect(defaultEventCalendar([])).toBeUndefined();
  });

  it("lists each account's primary calendar ahead of the rest, keeping the listed order otherwise", () => {
    const listed = [
      calendar("family", "joel@example.com"),
      calendar("joel", "joel@example.com", { primary: true }),
      calendar("team", "work@example.com"),
      calendar("work", "work@example.com", { primary: true }),
    ];
    expect(primaryCalendarsFirst(listed).map((option) => option.id)).toEqual(["joel", "work", "family", "team"]);
    expect(primaryCalendarsFirst([])).toEqual([]);
  });

  it("keeps To Do and In Progress tasks of the shown accounts, soonest due first", () => {
    const tasks = [
      task("undated"),
      task("later", { dueKind: "date", dueValue: "2026-10-30" }),
      task("doing", { status: "in_progress", dueKind: "date", dueValue: "2026-10-01" }),
      task("done", { status: "completed" }),
      task("cancelled", { status: "cancelled" }),
      task("elsewhere", { accountId: "work@example.com" }),
    ];
    expect(openTasksFor(tasks, ["joel@example.com"]).map((item) => item.id)).toEqual(["doing", "later", "undated"]);
    expect(openTasksFor(tasks, [])).toEqual([]);
  });

  it("offers open note tasks for booking only when asked", () => {
    const chore = noteTask("Draft memo");
    const done = noteTask("Filed", { checked: true, markerId: "m2" });
    const tasks = [task("undated"), chore, done];
    expect(openTasksFor(tasks, ["joel@example.com"]).map((item) => item.title)).toEqual(["undated"]);
    expect(openTasksFor(tasks, ["joel@example.com"], { includeNoteTasks: true }).map((item) => item.title).sort()).toEqual(["Draft memo", "undated"]);
    expect(openTasksFor(tasks, [], { includeNoteTasks: true }).map((item) => item.title)).toEqual(["Draft memo"]);
  });

  it("books a note task under its Tasks id and finds its block and task again", () => {
    const chore = noteTask("Draft memo");
    const start = new Date(2026, 10, 16, 10, 0);
    const request = taskBlockRequest(chore, calendar("primary", "joel@example.com"), start);
    expect(request.sourceTaskId).toBe("note:daily:2026-11-16:m1");
    expect(bookedSource(request.sourceTaskId)).toEqual({ kind: "noteTask", noteId: "daily:2026-11-16", markerId: "m1" });
    const booked = block("chore", "2026-11-16T15:00:00Z", "2026-11-16T15:30:00Z", request.sourceTaskId);
    const now = new Date("2026-11-16T12:00:00Z");
    expect(bookedBlock(chore, [booked], now)?.id).toBe("chore");
    // Another marker in the same note, or a replicated task, isn't this task.
    expect(bookedBlock(noteTask("Other", { markerId: "m9" }), [booked], now)).toBeNull();
    expect(bookedBlock(task("m1"), [booked], now)).toBeNull();
    expect(bookedTaskFor(booked, [task("proposal"), chore])?.title).toBe("Draft memo");
  });

  it("keeps old events booked for replicated tasks linked to them", () => {
    const proposal = task("proposal");
    const old = block("old", "2026-10-28T09:00:00Z", "2026-10-28T09:30:00Z", " proposal ");
    expect(bookedSource(old.sourceTaskId)).toEqual({ kind: "task", id: "proposal" });
    expect(bookedTaskFor(old, [noteTask("Draft memo"), proposal])).toBe(proposal);
    expect(bookedTaskFor(block("unlinked", "2026-10-28T09:00:00Z", "2026-10-28T09:30:00Z", null), [proposal])).toBeUndefined();
    expect(bookedSource("  ")).toBeNull();
  });

  it("asks for a thirty-minute block tagged with the task", () => {
    const start = new Date(2026, 9, 27, 14, 0);
    expect(TASK_BLOCK_MINUTES).toBe(30);
    expect(taskBlockRequest(task("proposal", { title: " Write proposal " }), calendar("primary", "joel@example.com"), start)).toEqual({
      accountId: "joel@example.com", calendarId: "primary", title: "Write proposal",
      start: start.toISOString(), end: new Date(2026, 9, 27, 14, 30).toISOString(),
      description: "", attendees: [], sourceTaskId: "proposal",
    });
  });

  it("names the next booked block still to end, else the latest one", () => {
    const proposal = task("proposal");
    const now = new Date("2026-10-27T12:00:00Z");
    const past = block("past", "2026-10-26T09:00:00Z", "2026-10-26T09:30:00Z");
    const next = block("next", "2026-10-28T09:00:00Z", "2026-10-28T09:30:00Z");
    const after = block("after", "2026-10-29T09:00:00Z", "2026-10-29T09:30:00Z");
    const other = block("other", "2026-10-27T13:00:00Z", "2026-10-27T13:30:00Z", "budget");
    expect(bookedBlock(proposal, [after, other, past, next], now)?.id).toBe("next");
    expect(bookedBlock(proposal, [past, other], now)?.id).toBe("past");
    expect(bookedBlock(proposal, [other, block("untagged", "2026-10-28T09:00:00Z", "2026-10-28T09:30:00Z", null)], now)).toBeNull();
  });
});
