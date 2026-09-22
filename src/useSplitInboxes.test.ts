import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { mailClient } from "./data/client";
import type { SplitInbox } from "./domain";
import { useSplitInboxes } from "./useSplitInboxes";

function splitInbox(id: string, name: string, sortOrder: number): SplitInbox {
  return { id, name, matchKind: "pattern", matchValue: name.toLowerCase(), sortOrder, createdAt: "2026-09-01T00:00:00Z", accountId: "you@example.com" };
}

describe("useSplitInboxes", () => {
  afterEach(() => vi.restoreAllMocks());

  it("marks the catalog loaded even when the first listing fails", async () => {
    vi.spyOn(mailClient, "listSplitInboxes").mockRejectedValue(new Error("offline"));
    const { result } = renderHook(() => useSplitInboxes());

    await waitFor(() => expect(result.current.loaded).toBe(true));
    expect(result.current.splitInboxes).toEqual([]);
  });

  it("applies create, rename, and delete results to the catalog", async () => {
    const product = splitInbox("split-1", "Product", 0);
    vi.spyOn(mailClient, "listSplitInboxes").mockResolvedValue([product]);
    vi.spyOn(mailClient, "createSplitInbox").mockResolvedValue(splitInbox("split-2", "Billing", 1));
    vi.spyOn(mailClient, "updateSplitInbox").mockResolvedValue({ ...product, name: "Roadmap" });
    vi.spyOn(mailClient, "deleteSplitInbox").mockResolvedValue();
    const { result } = renderHook(() => useSplitInboxes());
    await waitFor(() => expect(result.current.splitInboxes).toEqual([product]));

    await act(async () => { await result.current.create("Billing", "pattern", "billing", "you@example.com"); });
    expect(result.current.splitInboxes.map((item) => item.name)).toEqual(["Product", "Billing"]);

    await act(async () => { await result.current.rename("split-1", "Roadmap"); });
    expect(result.current.splitInboxes.map((item) => item.name)).toEqual(["Roadmap", "Billing"]);

    await act(async () => { await result.current.remove("split-1"); });
    expect(result.current.splitInboxes.map((item) => item.id)).toEqual(["split-2"]);
  });

  it("reloads the persisted order after reordering", async () => {
    const first = splitInbox("split-1", "Product", 0);
    const second = splitInbox("split-2", "Billing", 1);
    const listSplitInboxes = vi.spyOn(mailClient, "listSplitInboxes").mockResolvedValue([first, second]);
    vi.spyOn(mailClient, "reorderSplitInboxes").mockResolvedValue();
    const { result } = renderHook(() => useSplitInboxes());
    await waitFor(() => expect(result.current.splitInboxes).toHaveLength(2));

    listSplitInboxes.mockResolvedValue([{ ...second, sortOrder: 0 }, { ...first, sortOrder: 1 }]);
    await act(async () => { await result.current.reorder(["split-2", "split-1"]); });

    expect(mailClient.reorderSplitInboxes).toHaveBeenCalledWith(["split-2", "split-1"]);
    expect(result.current.splitInboxes.map((item) => item.id)).toEqual(["split-2", "split-1"]);
  });
});
