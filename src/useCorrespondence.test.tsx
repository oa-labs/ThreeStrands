import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Draft, OutboxItem } from "./correspondence";
import { mailClient } from "./data/client";
import { useCorrespondence } from "./useCorrespondence";

const draft: Draft = {
  id: "draft-1",
  revision: 2,
  account: "me@example.com",
  mode: "new",
  sourceId: null,
  threadId: null,
  replyId: null,
  references: [],
  to: "",
  cc: "",
  bcc: "",
  subject: "",
  body: "",
  attachments: [],
  updatedAt: 0,
};

const outbox: OutboxItem = {
  id: "outbox-1",
  draft,
  state: "undo_pending",
  deadline: Date.now() + 10_000,
  error: null,
};

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe("useCorrespondence", () => {
  it("loads drafts and outbox and starts each compose mode through the client", async () => {
    vi.spyOn(mailClient, "listDrafts").mockResolvedValue([draft]);
    vi.spyOn(mailClient, "listOutbox").mockResolvedValue([]);
    const createDraft = vi.spyOn(mailClient, "createDraft").mockImplementation(async (mode, sourceId) => ({
      ...draft,
      mode,
      sourceId: sourceId ?? null,
    }));
    const { result } = renderHook(() => useCorrespondence([], "message-1", "account@example.com"));

    await waitFor(() => expect(result.current.draftCount).toBe(1));
    act(() => result.current.context.compose());
    await waitFor(() => expect(result.current.activeDraft?.mode).toBe("new"));
    expect(createDraft).toHaveBeenCalledWith("new");

    act(() => result.current.context.openInbox());
    expect(result.current.activeDraft).toBeNull();
    act(() => result.current.context.reply());
    await waitFor(() => expect(createDraft).toHaveBeenLastCalledWith("reply", "message-1", "account@example.com"));
  });

  it("cancels the pending outbox send and refreshes the lists", async () => {
    vi.spyOn(mailClient, "listDrafts").mockResolvedValue([]);
    const listOutbox = vi.spyOn(mailClient, "listOutbox").mockResolvedValue([outbox]);
    const cancelSend = vi.spyOn(mailClient, "cancelSend").mockResolvedValue(draft);
    const { result } = renderHook(() => useCorrespondence([]));
    await waitFor(() => expect(result.current.context.canUndoSend).toBe(true));

    act(() => result.current.context.undoSend());
    await waitFor(() => expect(cancelSend).toHaveBeenCalledWith("outbox-1"));
    expect(listOutbox).toHaveBeenCalledTimes(2);
    expect(result.current.activeDraft).toEqual(draft);
  });

  it("guards duplicate recovery actions until the first request settles", async () => {
    vi.spyOn(mailClient, "listDrafts").mockResolvedValue([]);
    vi.spyOn(mailClient, "listOutbox").mockResolvedValue([]);
    let resolveRecovery!: (value: Draft) => void;
    const recoverSend = vi.spyOn(mailClient, "recoverSend").mockReturnValue(new Promise((resolve) => { resolveRecovery = resolve; }));
    const { result } = renderHook(() => useCorrespondence([]));

    act(() => {
      result.current.restoreFailedSend("outbox-1");
      result.current.restoreFailedSend("outbox-1");
    });
    expect(recoverSend).toHaveBeenCalledTimes(1);
    expect(result.current.pendingOutboxActions.has("outbox-1")).toBe(true);

    await act(async () => resolveRecovery(draft));
    await waitFor(() => expect(result.current.pendingOutboxActions.has("outbox-1")).toBe(false));
  });
});
