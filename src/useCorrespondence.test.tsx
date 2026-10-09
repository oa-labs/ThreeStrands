import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Draft, OutboxItem } from "./correspondence";
import { mailClient } from "./data/client";
import type { Snippet } from "./domain";
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

const noSnippets: Snippet[] = [];
const noopCreateSnippet = vi.fn();
const noopUpdateSnippet = vi.fn();
const noopDeleteSnippet = vi.fn();
const snippetArgs = [noSnippets, noopCreateSnippet, noopUpdateSnippet, noopDeleteSnippet] as const;

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
    const { result } = renderHook(() => useCorrespondence([], "message-1", "account@example.com", ...snippetArgs, "thread-1"));

    await waitFor(() => expect(result.current.draftCount).toBe(1));
    act(() => result.current.context.compose());
    await waitFor(() => expect(result.current.activeDraft?.mode).toBe("new"));
    expect(createDraft).toHaveBeenCalledWith("new");

    act(() => result.current.context.openInbox());
    expect(result.current.activeDraft).toBeNull();
    act(() => result.current.context.reply());
    await waitFor(() => expect(createDraft).toHaveBeenLastCalledWith("reply", "message-1", "account@example.com"));
  });

  it.each(["reply", "replyAll"] as const)("starts a %s to the selected message with only the selected quote", async (mode) => {
    vi.spyOn(mailClient, "listDrafts").mockResolvedValue([]);
    vi.spyOn(mailClient, "listOutbox").mockResolvedValue([]);
    const createDraft = vi.spyOn(mailClient, "createDraft").mockResolvedValue({
      ...draft, revision: 0, mode, body: "\n\nOn Tuesday, Sender wrote:\n> Full message", bodyHtml: "",
    });
    const saveDraft = vi.spyOn(mailClient, "saveDraft").mockImplementation(async (next) => ({ ...next, revision: 1 }));
    const { result } = renderHook(() => useCorrespondence([], "latest", "account@example.com", ...snippetArgs, "thread-1"));

    act(() => result.current.context[mode]("older", "One line\nAnother line"));
    await waitFor(() => expect(result.current.activeDraft?.revision).toBe(1));
    expect(createDraft).toHaveBeenCalledWith(mode, "older", "account@example.com");
    expect(saveDraft.mock.calls[0][0].body).toBe("\n\nOn Tuesday, Sender wrote:\n> One line\n> Another line");
  });

  it("starts a forward to the selected message with only the selected quote", async () => {
    vi.spyOn(mailClient, "listDrafts").mockResolvedValue([]);
    vi.spyOn(mailClient, "listOutbox").mockResolvedValue([]);
    const header = "---------- Forwarded message ----------\nFrom: Sender\nDate: Tuesday\nSubject: Subject\nTo: Me\n\n";
    const createDraft = vi.spyOn(mailClient, "createDraft").mockResolvedValue({
      ...draft, revision: 0, mode: "forward", body: "", forwardedContent: { html: "<p>Full message</p>", text: header + "Full message" },
    });
    const saveDraft = vi.spyOn(mailClient, "saveDraft").mockImplementation(async (next) => ({ ...next, revision: 1 }));
    const { result } = renderHook(() => useCorrespondence([], "latest", "account@example.com", ...snippetArgs, "thread-1"));

    act(() => result.current.context.forward("older", "One line\nAnother line"));
    await waitFor(() => expect(result.current.activeDraft?.revision).toBe(1));
    expect(createDraft).toHaveBeenCalledWith("forward", "older", "account@example.com");
    expect(saveDraft.mock.calls[0][0].forwardedContent?.text).toBe(header + "> One line\n> Another line");
  });

  it("cancels the pending outbox send and refreshes the lists", async () => {
    // Fake timers keep the one-second outbox poll from firing on its own, so
    // every listOutbox call below comes from a load or an explicit refresh.
    vi.useFakeTimers();
    const flush = () => act(async () => { await vi.advanceTimersByTimeAsync(0); });
    vi.spyOn(mailClient, "listDrafts").mockResolvedValue([]);
    const listOutbox = vi.spyOn(mailClient, "listOutbox").mockResolvedValue([outbox]);
    const cancelSend = vi.spyOn(mailClient, "cancelSend").mockResolvedValue(draft);
    const { result } = renderHook(() => useCorrespondence([], undefined, undefined, ...snippetArgs, null));
    await flush();
    expect(result.current.context.canUndoSend).toBe(true);
    expect(listOutbox).toHaveBeenCalledTimes(1);

    act(() => result.current.context.undoSend());
    await flush();
    expect(cancelSend).toHaveBeenCalledTimes(1);
    expect(cancelSend).toHaveBeenCalledWith("outbox-1");
    expect(listOutbox).toHaveBeenCalledTimes(2);
    expect(result.current.activeDraft).toEqual(draft);

    // Control: the poll is what would otherwise have added calls.
    await act(async () => { await vi.advanceTimersByTimeAsync(1000); });
    expect(listOutbox).toHaveBeenCalledTimes(3);
  });

  it("guards duplicate recovery actions until the first request settles", async () => {
    vi.spyOn(mailClient, "listDrafts").mockResolvedValue([]);
    vi.spyOn(mailClient, "listOutbox").mockResolvedValue([]);
    let resolveRecovery!: (value: Draft) => void;
    const recoverSend = vi.spyOn(mailClient, "recoverSend").mockReturnValue(new Promise((resolve) => { resolveRecovery = resolve; }));
    const { result } = renderHook(() => useCorrespondence([], undefined, undefined, ...snippetArgs, null));

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
