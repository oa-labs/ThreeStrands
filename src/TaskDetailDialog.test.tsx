import { describe, expect, it } from "vitest";
import type { ThreadTask } from "./domain";
import { taskDetailChanges, taskDetailDrafts } from "./TaskDetailDialog";

const task: ThreadTask = {
  id: "plan", accountId: "you@example.com", threadId: null, sourceMessageId: null, subjectSnapshot: null,
  title: "Plan launch", notes: null, kind: "follow_up", dueKind: "none", dueValue: null, timeZone: null,
  repeatIntervalDays: null, status: "open", completionSource: null, evidenceText: null, waitAfter: null,
  createdAt: "2026-09-19T10:00:00Z", updatedAt: "2026-09-19T10:00:00Z", completedAt: null,
};

describe("taskDetailChanges", () => {
  it("sends nothing when the drafts match the task", () => {
    expect(taskDetailChanges(task, taskDetailDrafts(task))).toEqual({ request: {}, errors: [] });
    const dated = { ...task, dueKind: "datetime" as const, dueValue: "2030-10-02T05:30:00.000Z", timeZone: "Asia/Tokyo" };
    expect(taskDetailChanges(dated, taskDetailDrafts(dated))).toEqual({ request: {}, errors: [] });
  });

  it("accepts repeat intervals from 1 through 365 days, the task store's limit, and rejects values outside them", () => {
    const repeat = (value: string) => taskDetailChanges(task, { ...taskDetailDrafts(task), repeatIntervalDays: value });
    expect(repeat("0").errors).toEqual(["Repeat every 1 to 365 days."]);
    expect(repeat("1").request).toEqual({ repeatIntervalDays: 1 });
    expect(repeat("365").request).toEqual({ repeatIntervalDays: 365 });
    expect(repeat("366").errors).toEqual(["Repeat every 1 to 365 days."]);
    expect(repeat("2.5").request).toEqual({});
    expect(repeat("").request).toEqual({});
  });

  it("still saves valid fields when another field is invalid or incomplete", () => {
    const drafts = { ...taskDetailDrafts(task), notes: "  Outline  ", dueKind: "datetime" as const, dueValue: "2030-10-02T14:30", timeZone: "Mars/Olympus" };
    expect(taskDetailChanges(task, drafts)).toEqual({
      request: { notes: "Outline" },
      errors: ["Choose a valid timezone, such as America/New_York."],
    });
    // A due type picked without a date waits for the date instead of saving half a schedule.
    expect(taskDetailChanges(task, { ...taskDetailDrafts(task), dueKind: "date", dueValue: "" })).toEqual({ request: {}, errors: [] });
  });

  it("ignores the repeat interval for task types that cannot repeat", () => {
    const action = { ...task, kind: "action" as const };
    expect(taskDetailChanges(action, { ...taskDetailDrafts(action), repeatIntervalDays: "0" })).toEqual({ request: {}, errors: [] });
  });
});
