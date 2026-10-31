import { act, renderHook } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { mailClient } from "./data/client";
import { useConversationSnooze } from "./useConversationSnooze";
import type { CommandResult } from "./commands";
import type { ConversationSnooze } from "./domain";

const until = "2099-01-01T09:00:00Z";
describe("useConversationSnooze", () => {
  it("undoes a batch to its original archive state, retaining an earlier snooze", async () => {
    const threads = (await mailClient.listThreads()).slice(0, 2);
    const saved: ConversationSnooze[] = threads.map((thread) => ({ id: thread.id, accountId: thread.accountId, providerThreadId: thread.providerThreadId, until, state: "active", messageIds: [] }));
    const before = { ...saved[0], until: "2098-01-01T09:00:00Z" };
    vi.spyOn(mailClient, "listSnoozeRecords").mockResolvedValue([before]);
    vi.spyOn(mailClient, "snoozeThreads").mockResolvedValue(saved);
    const restore = vi.spyOn(mailClient, "restoreSnoozes").mockResolvedValue();
    const refresh = vi.fn().mockResolvedValue(undefined);
    const { result } = renderHook(() => useConversationSnooze(refresh));
    let response: CommandResult = {};
    await act(async () => { response = await result.current.snooze(threads.map((thread, index) => ({ ...thread, archived: index === 0 })), until); });
    expect(response.message).toBe("Snoozed 2 conversations");
    await act(async () => { await response.undoAction?.(); });
    expect(restore).toHaveBeenCalledWith([before, { ...saved[1], state: "cancelled" }], [threads[0].id]);
    expect(refresh).toHaveBeenCalledTimes(2);
  });
  it("keeps a failed native operation from refreshing or creating an undo", async () => {
    vi.spyOn(mailClient, "listSnoozeRecords").mockResolvedValue([]);
    vi.spyOn(mailClient, "snoozeThreads").mockRejectedValue(new Error("Database busy"));
    const refresh = vi.fn();
    const { result } = renderHook(() => useConversationSnooze(refresh));
    await expect(result.current.snooze([], until)).rejects.toThrow("Database busy");
    expect(refresh).not.toHaveBeenCalled();
  });
});
