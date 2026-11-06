import { act, cleanup, renderHook } from "@testing-library/react";
import { useState } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mailClient } from "./data/client";
import type { Thread, ThreadDetail } from "./domain";
import type { CommandResult } from "./commands";
import { useThreadMutations } from "./useThreadMutations";

function conversation(id: string, unread: boolean): Thread {
  return {
    id, providerThreadId: id, accountId: "person@example.com", subject: id, snippet: "",
    participants: [], lastMessageAt: "2026-11-01T12:00:00Z", lastReceivedAt: "2026-11-01T12:00:00Z",
    unread, archived: false, trashed: false, starred: false, labels: ["INBOX"],
    summary: null, summaryGeneratedAt: null, summaryRevision: null, hasAttachments: false,
  };
}

function setup(markReadOnArchive: boolean, selectedId: string | null = null, markReadOnOpen?: boolean) {
  const initial = [conversation("unread", true), conversation("read", false)];
  const initialDetail = {
    thread: initial[0], messages: [{ id: "m1", unread: true }, { id: "m2", unread: true }],
  } as ThreadDetail;
  const reload = vi.fn().mockResolvedValue(undefined);
  const hook = renderHook(() => {
    const [threads, setThreads] = useState(initial);
    const [detail, setDetail] = useState<ThreadDetail | null>(initialDetail);
    const mutate = useThreadMutations({
      threads, detail, visibleDetail: detail, selectedId,
      setThreads, setDetail, setSelectedId: vi.fn(), setCheckedIds: vi.fn(),
      setSyncStatus: vi.fn(), setNotice: vi.fn(), refreshMailboxUnreadCounts: vi.fn(),
      mailbox: "allMail", activeAccountId: null, activeSplitInboxId: null,
      includeArchived: true, isThreadMailbox: true, autoReadDelaySeconds: 60, markReadOnArchive, markReadOnOpen,
      triageSessionRef: { current: null }, recordTriageEvent: vi.fn(),
      loadThreadsRef: { current: reload }, queryRef: { current: "" },
    });
    return { mutate, threads, detail };
  });
  return hook;
}

describe("mark read after opening", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.spyOn(mailClient, "mutateThreads").mockResolvedValue(undefined);
    vi.spyOn(mailClient, "flushPending").mockResolvedValue({} as never);
  });
  afterEach(() => { cleanup(); vi.restoreAllMocks(); vi.useRealTimers(); });

  it("marks the open conversation read once the delay passes", async () => {
    const { result } = setup(false, "unread");
    await act(async () => { await vi.advanceTimersByTimeAsync(60_000); });
    expect(result.current.threads[0].unread).toBe(false);
  });

  it("leaves the open conversation unread when set to Never", async () => {
    const { result } = setup(false, "unread", false);
    await act(async () => { await vi.advanceTimersByTimeAsync(120_000); });
    expect(result.current.threads[0].unread).toBe(true);
    expect(mailClient.mutateThreads).not.toHaveBeenCalled();
  });
});

describe("archive read status", () => {
  beforeEach(() => {
    vi.spyOn(mailClient, "mutateThreads").mockResolvedValue(undefined);
    vi.spyOn(mailClient, "flushPending").mockResolvedValue({} as never);
  });
  afterEach(() => { cleanup(); vi.restoreAllMocks(); });

  it("preserves unread status with the preference off", async () => {
    const { result } = setup(false);
    await act(async () => { await result.current.mutate(["unread"], { kind: "archive", value: true }); });
    expect(result.current.threads[0]).toMatchObject({ archived: true, unread: true });
    expect(mailClient.mutateThreads).toHaveBeenCalledWith([{ kind: "archive", threadId: "unread", value: true }]);
  });

  it.each([null, "unread"])("marks bulk archives read before the timer, with selection %s, and Undo restores mixed read states", async (selection) => {
    const { result } = setup(true, selection);
    let command!: CommandResult;
    await act(async () => { command = await result.current.mutate(["unread", "read"], { kind: "archive", value: true }); });
    expect(result.current.threads.every((thread) => thread.archived && !thread.unread)).toBe(true);
    expect(result.current.detail?.messages.every((message) => !message.unread)).toBe(true);
    expect(mailClient.mutateThreads).toHaveBeenCalledWith([
      { kind: "read", threadId: "unread", value: true }, { kind: "archive", threadId: "unread", value: true },
      { kind: "read", threadId: "read", value: true }, { kind: "archive", threadId: "read", value: true },
    ]);
    await act(async () => { await command.undoAction?.(); });
    expect(mailClient.mutateThreads).toHaveBeenLastCalledWith([
      { kind: "archive", threadId: "unread", value: false }, { kind: "read", threadId: "unread", value: false },
      { kind: "archive", threadId: "read", value: false },
    ]);
    expect(result.current.threads.find((thread) => thread.id === "unread")).toMatchObject({ archived: false, unread: true });
    expect(result.current.threads.find((thread) => thread.id === "read")).toMatchObject({ archived: false, unread: false });
  });

  it("restores unloaded conversations' unread status on Undo too", async () => {
    vi.spyOn(mailClient, "getThread").mockResolvedValue({ thread: conversation("unloaded", true), messages: [] } as unknown as ThreadDetail);
    const { result } = setup(true);
    let command!: CommandResult;
    await act(async () => { command = await result.current.mutate(["unloaded"], { kind: "archive", value: true }, { includeUnloaded: true }); });
    await act(async () => { await command.undoAction?.(); });
    expect(mailClient.mutateThreads).toHaveBeenLastCalledWith([
      { kind: "archive", threadId: "unloaded", value: false }, { kind: "read", threadId: "unloaded", value: false },
    ]);
  });

  it("rolls back both read status and location when saving fails", async () => {
    vi.mocked(mailClient.mutateThreads).mockRejectedValueOnce(new Error("failed"));
    const { result } = setup(true);
    await act(async () => { await result.current.mutate(["unread"], { kind: "archive", value: true }); });
    expect(result.current.threads.find((thread) => thread.id === "unread")).toMatchObject({ archived: false, unread: true });
    expect(result.current.detail?.messages.every((message) => message.unread)).toBe(true);
  });

  it.each(["archive", "trash", "spam"] as const)("does not mark read for %s with value false", async (kind) => {
    const { result } = setup(true);
    await act(async () => { await result.current.mutate(["unread"], { kind, value: false }); });
    expect(mailClient.mutateThreads).toHaveBeenCalledWith([{ kind, threadId: "unread", value: false }]);
  });

  it.each(["trash", "spam"] as const)("does not extend the archive preference to %s", async (kind) => {
    const { result } = setup(true);
    await act(async () => { await result.current.mutate(["unread"], { kind, value: true }); });
    expect(mailClient.mutateThreads).toHaveBeenCalledWith([{ kind, threadId: "unread", value: true }]);
  });
});
