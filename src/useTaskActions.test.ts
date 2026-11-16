import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { mailClient } from "./data/client";
import type { ThreadDetail, ThreadTask } from "./domain";
import { useTaskActions } from "./useTaskActions";

function task(id: string, overrides: Partial<ThreadTask> = {}): ThreadTask {
  return {
    id, accountId: "you@example.com", threadId: null, sourceMessageId: null, subjectSnapshot: null, title: `Task ${id}`, notes: null,
    kind: "action", dueKind: "none", dueValue: null, timeZone: null, repeatIntervalDays: null, status: "open", completionSource: null,
    evidenceText: null, waitAfter: null, createdAt: "2026-10-01T00:00:00Z", updatedAt: "2026-10-01T00:00:00Z", completedAt: null, ...overrides,
  };
}

function renderActions() {
  const setNotice = vi.fn();
  const options = {
    correspondence: { replyWithFollowUp: vi.fn() }, setSelectedId: vi.fn(), setDetail: vi.fn(), accounts: [], activeAccountId: null,
    rightWorkspace: "today", visibleDetail: null, selectedId: null, setNotice, setTaskRevision: vi.fn(), refreshTaskIndicators: vi.fn(async () => {}),
    updateActionProposal: vi.fn(), removeActionProposal: vi.fn(),
  } as unknown as Parameters<typeof useTaskActions>[0];
  return { ...renderHook(() => useTaskActions(options)), setNotice };
}

const values = { title: "Draft the proposal", notes: null, kind: "action" as const, dueKind: "none" as const, dueValue: null, timeZone: null, repeatIntervalDays: null };

describe("useTaskActions: a new task for a caller", () => {
  afterEach(() => vi.restoreAllMocks());

  it("opens the editor for a goal and resolves with the task once it saves, keeping the goal", async () => {
    vi.spyOn(mailClient, "listGoals").mockResolvedValue([]);
    const createTask = vi.spyOn(mailClient, "createTask").mockResolvedValue(task("made", { goalId: "g1" }));
    const { result } = renderActions();
    let created!: Promise<ThreadTask | null>;
    act(() => { created = result.current.createTaskInEditor({ kind: "goal", goalId: "g1", accountId: "you@example.com" }); });
    expect(result.current.taskEditor).toMatchObject({ kind: "goal", goalId: "g1", accountId: "you@example.com" });
    expect(result.current.taskEditorAccountId).toBe("you@example.com");
    await act(async () => { await result.current.submitTaskEditor(values); });
    expect(createTask).toHaveBeenCalledWith({ accountId: "you@example.com", threadId: null, subjectSnapshot: null, ...values, goalId: "g1" });
    await expect(created).resolves.toMatchObject({ id: "made" });
    expect(result.current.taskEditor).toBeNull();
  });

  it("respects a goal the user cleared in the form", async () => {
    vi.spyOn(mailClient, "listGoals").mockResolvedValue([]);
    const createTask = vi.spyOn(mailClient, "createTask").mockResolvedValue(task("made"));
    const { result } = renderActions();
    act(() => { void result.current.createTaskInEditor({ kind: "goal", goalId: "g1", accountId: "you@example.com" }); });
    await act(async () => { await result.current.submitTaskEditor({ ...values, goalId: null }); });
    expect(createTask).toHaveBeenCalledWith(expect.objectContaining({ goalId: null }));
  });

  it("opens the editor from a conversation, linked to it, and resolves null without creating anything on Cancel", async () => {
    const thread = { thread: { id: "th9", accountId: "you@example.com", subject: "Board deck" }, messages: [{ id: "m1", bodyText: "Can you send it?" }] } as unknown as ThreadDetail;
    vi.spyOn(mailClient, "getThread").mockResolvedValue(thread);
    vi.spyOn(mailClient, "listGoals").mockResolvedValue([]);
    const createTask = vi.spyOn(mailClient, "createTask").mockResolvedValue(task("made", { threadId: "th9" }));
    const { result } = renderActions();

    let cancelled!: Promise<ThreadTask | null>;
    act(() => { cancelled = result.current.createTaskInEditor({ kind: "conversation", threadId: "th9", accountId: "you@example.com" }); });
    await waitFor(() => expect(result.current.taskEditor).toMatchObject({ kind: "new" }));
    act(() => result.current.closeTaskEditor());
    await expect(cancelled).resolves.toBeNull();
    expect(createTask).not.toHaveBeenCalled();

    let saved!: Promise<ThreadTask | null>;
    act(() => { saved = result.current.createTaskInEditor({ kind: "conversation", threadId: "th9", accountId: "you@example.com" }); });
    await waitFor(() => expect(result.current.taskEditor).toMatchObject({ kind: "new" }));
    await act(async () => { await result.current.submitTaskEditor(values); });
    expect(createTask).toHaveBeenCalledWith(expect.objectContaining({ accountId: "you@example.com", threadId: "th9", sourceMessageId: "m1", subjectSnapshot: "Board deck" }));
    await expect(saved).resolves.toMatchObject({ id: "made" });
  });

  it("resolves null and says why when the conversation can't be read", async () => {
    vi.spyOn(mailClient, "getThread").mockRejectedValue(new Error("Not on this device"));
    const { result, setNotice } = renderActions();
    let pending!: Promise<ThreadTask | null>;
    act(() => { pending = result.current.createTaskInEditor({ kind: "conversation", threadId: "gone", accountId: "you@example.com" }); });
    await expect(pending).resolves.toBeNull();
    expect(setNotice).toHaveBeenCalledWith({ message: "Not on this device" });
  });
});
