import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mailClient } from "./data/client";
import { planItemKey } from "./dailyPlan";
import type { DailyPlan, DailyPlanItem, Note, PlanReview, ThreadTask } from "./domain";
import { addNewTask, readRecovery, startRoutine, writeRecovery } from "./planRoutine";
import { reviewHasContent, usePlanRoutine } from "./usePlanRoutine";

const DAY = "2026-11-17";
const ref = (id: string) => ({ version: 1 as const, kind: "task" as const, id });
const item = (id: string): DailyPlanItem => ({ key: planItemKey(ref(id)), kind: "task", reference: ref(id), title: `Task ${id}`, state: "open", marker: null, taskId: id, accountId: "me@example.com", goal: null, contactId: null, carryCount: 0, start: 0, end: 0 });
const plan = (items: DailyPlanItem[], revision: string | null = "r1"): DailyPlan => ({ date: DAY, noteId: `daily:${DAY}`, revision, items, notToday: [] });
const emptyReview: PlanReview = { date: DAY, reviewed: null, done: [], alsoDone: [], meetings: null, didntGetTo: [] };
const note = (updatedAt: string): Note => ({ id: `daily:${DAY}`, title: DAY, body: "", dailyDate: DAY, createdAt: updatedAt, updatedAt, trashed: false, trashedAt: null });
const task = (id: string): ThreadTask => ({ id, accountId: "me@example.com", threadId: null, subjectSnapshot: null, title: `Task ${id}`, kind: "action", dueKind: "none", status: "open", createdAt: "", updatedAt: "" });

beforeEach(() => {
  vi.spyOn(mailClient, "listTasks").mockResolvedValue([task("a"), task("b")]);
  vi.spyOn(mailClient, "listNoteTasks").mockResolvedValue([]);
});
afterEach(() => { vi.restoreAllMocks(); window.localStorage.clear(); });

describe("usePlanRoutine", () => {
  it("starts at Choose when there is nothing to review, and at Review otherwise", async () => {
    vi.spyOn(mailClient, "dailyPlan").mockResolvedValue(plan([]));
    const review = vi.spyOn(mailClient, "planReview").mockResolvedValue(emptyReview);
    const { result, unmount } = renderHook(() => usePlanRoutine(DAY));
    await waitFor(() => expect(result.current.state).not.toBeNull());
    expect(result.current.state?.step).toBe("choose");
    expect(result.current.restored).toBeNull();
    unmount();

    const withLeftovers = { ...emptyReview, reviewed: "2026-11-16", didntGetTo: [{ ...item("a"), plannedOn: "2026-11-16" }] };
    expect(reviewHasContent(withLeftovers)).toBe(true);
    expect(reviewHasContent(emptyReview)).toBe(false);
    review.mockResolvedValue(withLeftovers);
    const next = renderHook(() => usePlanRoutine(DAY));
    await waitFor(() => expect(next.result.current.state?.step).toBe("review"));
  });

  it("keeps choices on this device as they change and restores them, re-based, with Discard", async () => {
    vi.spyOn(mailClient, "dailyPlan").mockResolvedValue(plan([item("a")], "r2"));
    vi.spyOn(mailClient, "planReview").mockResolvedValue(emptyReview);
    const kept = addNewTask({ ...startRoutine(DAY, plan([item("a")])), step: "fit" }, "Draft memo");
    writeRecovery(kept);
    const { result } = renderHook(() => usePlanRoutine(DAY));
    await waitFor(() => expect(result.current.state).not.toBeNull());
    expect(result.current.restored).toBe(kept.savedAt);
    expect(result.current.state?.step).toBe("fit");
    expect(result.current.state?.base).toBe("r2");
    expect(result.current.state?.items.map((entry) => entry.title)).toEqual(["Task a", "Draft memo"]);

    act(() => result.current.update((state) => addNewTask(state, "Call Sam")));
    expect(readRecovery(DAY)?.items.map((entry) => entry.title)).toEqual(["Task a", "Draft memo", "Call Sam"]);

    act(() => result.current.discard());
    expect(result.current.restored).toBeNull();
    expect(result.current.state?.items.map((entry) => entry.title)).toEqual(["Task a"]);
    expect(readRecovery(DAY)).toBeNull();
  });

  it("saves every change at once with the routine's operation id, forgets the kept choices, and keeps them when refused", async () => {
    vi.spyOn(mailClient, "dailyPlan").mockResolvedValue(plan([item("a")]));
    vi.spyOn(mailClient, "planReview").mockResolvedValue(emptyReview);
    const save = vi.spyOn(mailClient, "saveDailyPlan").mockRejectedValueOnce(new Error("That day's note changed. Open it before saving the plan."))
      .mockResolvedValue({ operationId: "x", note: note("r3"), added: [], removed: [], notToday: [], already: [], changed: true });
    const { result } = renderHook(() => usePlanRoutine(DAY));
    await waitFor(() => expect(result.current.state).not.toBeNull());
    act(() => result.current.update((state) => addNewTask(state, "Draft memo")));

    await act(async () => { expect(await result.current.save()).toBeNull(); });
    expect(result.current.saveError).toMatch(/changed/);
    expect(readRecovery(DAY)?.items).toHaveLength(2);

    let saved: Awaited<ReturnType<typeof result.current.save>> = null;
    await act(async () => { saved = await result.current.save(); });
    expect(saved).not.toBeNull();
    const operationId = result.current.state!.operationId;
    expect(save).toHaveBeenNthCalledWith(1, DAY, "r1", operationId, { items: [{ kind: "existing", key: item("a").key }, { kind: "add", addition: { kind: "newTask", title: "Draft memo" }, carried: null }], remove: [], notToday: [] });
    // The retry reuses the operation id, so a save whose answer was lost isn't repeated.
    expect(save.mock.calls[1][2]).toBe(operationId);
    expect(readRecovery(DAY)).toBeNull();
  });

  it("marks restored items whose task is gone instead of saving them", async () => {
    vi.spyOn(mailClient, "dailyPlan").mockResolvedValue(plan([]));
    vi.spyOn(mailClient, "planReview").mockResolvedValue(emptyReview);
    const kept = startRoutine(DAY, plan([]));
    kept.items = [{ id: planItemKey(ref("gone")), key: planItemKey(ref("gone")), title: "Gone", existing: false, addition: { kind: "link", reference: ref("gone"), label: "Gone" }, carried: null, block: null }];
    writeRecovery(kept);
    const { result } = renderHook(() => usePlanRoutine(DAY));
    await waitFor(() => expect(result.current.state).not.toBeNull());
    expect(result.current.state?.items[0].missing).toBe(true);
  });
});
