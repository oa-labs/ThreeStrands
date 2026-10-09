import { act, cleanup, renderHook } from "@testing-library/react";
import { useState } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mailClient } from "./data/client";
import type { Thread } from "./domain";
import { useMailboxThreads } from "./useMailboxThreads";

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

async function flushReload() {
  await act(async () => { await vi.advanceTimersByTimeAsync(0); });
}

describe("useMailboxThreads", () => {
  let threads: Thread[];

  beforeEach(async () => {
    threads = await mailClient.listThreads();
    vi.useFakeTimers();
  });

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    vi.useRealTimers();
  });

  function setup(initialSelection: string | null = null) {
    const refreshUnreadCounts = vi.fn();
    const refreshMailboxUnreadCounts = vi.fn();
    const hook = renderHook(({ accountId }: { accountId: string }) => {
      const [selectedId, setSelectedId] = useState(initialSelection);
      const mailbox = useMailboxThreads({
        activeAccountId: accountId,
        mailbox: "inbox",
        activeSplitInboxId: null,
        setSelectedId,
        refreshUnreadCounts,
        refreshMailboxUnreadCounts,
      });
      return { ...mailbox, selectedId };
    }, { initialProps: { accountId: "first@example.com" } });
    return { ...hook, refreshUnreadCounts, refreshMailboxUnreadCounts };
  }

  it("invalidates an old account's response before the replacement reload starts", async () => {
    const oldPage = deferred<{ threads: Thread[]; hasMore: boolean }>();
    vi.spyOn(mailClient, "listThreadsPage")
      .mockReturnValueOnce(oldPage.promise)
      .mockResolvedValueOnce({ threads: [threads[1]!], hasMore: false });
    const { result, rerender, refreshUnreadCounts } = setup();
    await flushReload();

    rerender({ accountId: "second@example.com" });
    await act(async () => { oldPage.resolve({ threads: [threads[0]!], hasMore: true }); });
    expect(result.current.threads).toEqual([]);
    expect(result.current.selectedId).toBeNull();
    expect(refreshUnreadCounts).not.toHaveBeenCalled();

    await flushReload();
    expect(result.current.threads).toEqual([threads[1]]);
    expect(result.current.selectedId).toBe(threads[1]!.id);
    expect(result.current.hasMoreResults).toBe(false);
  });

  it("does not append an old page after switching accounts", async () => {
    const more = deferred<{ threads: Thread[]; hasMore: boolean }>();
    vi.spyOn(mailClient, "listThreadsPage")
      .mockResolvedValueOnce({ threads: [threads[0]!], hasMore: true })
      .mockReturnValueOnce(more.promise)
      .mockResolvedValueOnce({ threads: [threads[2]!], hasMore: false });
    const { result, rerender } = setup();
    await flushReload();
    let loading!: Promise<void>;
    act(() => { loading = result.current.loadMoreResults(); });

    rerender({ accountId: "second@example.com" });
    await flushReload();
    await act(async () => {
      more.resolve({ threads: [threads[1]!], hasMore: true });
      await loading;
    });
    expect(result.current.threads).toEqual([threads[2]]);
    expect(result.current.hasMoreResults).toBe(false);
    expect(result.current.loadingMoreState).toBe(false);
  });

  it("keeps a conversation opened by a context jump selected outside the current page", async () => {
    vi.spyOn(mailClient, "listThreadsPage").mockResolvedValue({ threads: [threads[0]!], hasMore: false });
    const { result } = setup("context-thread");
    result.current.contextOpenedThreadRef.current = "context-thread";
    await flushReload();
    expect(result.current.selectedId).toBe("context-thread");

    result.current.contextOpenedThreadRef.current = null;
    await act(async () => { await result.current.loadThreads(""); });
    expect(result.current.selectedId).toBe(threads[0]!.id);
  });
});
