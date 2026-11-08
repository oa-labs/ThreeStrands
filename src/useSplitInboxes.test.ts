import { saveSearch, readSavedSearches, SAVED_SEARCHES_KEY } from "./savedSearches";
import { MAIL_VIEW_ORDER_KEY, SAVED_VIEW_PREFIX, isSavedSearchView } from "./mailViews";
import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { mailClient } from "./data/client";
import type { SplitInbox } from "./domain";
import { useSplitInboxes } from "./useSplitInboxes";

function splitInbox(id: string, name: string, sortOrder: number): SplitInbox {
  return { id, name, matchKind: "pattern", matchValue: name.toLowerCase(), sortOrder, createdAt: "2026-09-01T00:00:00Z", accountId: "you@example.com" };
}

describe("useSplitInboxes", () => {
  afterEach(() => { vi.restoreAllMocks(); localStorage.removeItem(SAVED_SEARCHES_KEY); localStorage.removeItem(MAIL_VIEW_ORDER_KEY); });

  it("adopts existing saved searches into the shared catalog without changing data or routing splits", async () => {
    const [saved] = saveSearch({ name: "All invoices", query: "invoice", accountId: null, inboxOnly: false, messageFilters: ["starred"] });
    const legacy = localStorage.getItem(SAVED_SEARCHES_KEY);
    const routing = splitInbox("routing", "Routing", 0);
    vi.spyOn(mailClient, "listSplitInboxes").mockResolvedValue([routing]);
    const first = renderHook(() => useSplitInboxes());
    await waitFor(() => expect(first.result.current.loaded).toBe(true));
    expect(first.result.current.views).toEqual([routing, expect.objectContaining({ id: SAVED_VIEW_PREFIX + saved.id, accountId: null, savedSearch: saved })]);
    expect(first.result.current.splitInboxes).toEqual([routing]);
    expect(localStorage.getItem(SAVED_SEARCHES_KEY)).toBe(legacy);
    first.unmount();
    const second = renderHook(() => useSplitInboxes());
    await waitFor(() => expect(second.result.current.views).toHaveLength(2));
    expect(second.result.current.views[1].id).toBe(SAVED_VIEW_PREFIX + saved.id);
  });

  it("edits, renames, and deletes device-local saved views without changing routing rules", async () => {
    vi.spyOn(mailClient, "listSplitInboxes").mockResolvedValue([]);
    const nativeRename = vi.spyOn(mailClient, "updateSplitInbox");
    const nativeDelete = vi.spyOn(mailClient, "deleteSplitInbox");
    const { result } = renderHook(() => useSplitInboxes());
    await waitFor(() => expect(result.current.loaded).toBe(true));
    await act(async () => { await result.current.createSearch({ name: "Invoices", query: "invoice", accountId: "old@example.com", inboxOnly: true, messageFilters: ["unread"] }); });
    const id = result.current.views[0].id;
    await act(async () => { await result.current.rename(id, "Bills"); });
    await act(async () => { await result.current.updateSearch(id, { name: "Bills", query: "has:attachment", accountId: null, inboxOnly: false, messageFilters: ["starred"] }); });
    const view = result.current.views[0];
    expect(isSavedSearchView(view) && view.savedSearch).toEqual(expect.objectContaining({ name: "Bills", query: "has:attachment", accountId: null, inboxOnly: false, messageFilters: ["starred"] }));
    await act(async () => { await result.current.remove(id); });
    expect(readSavedSearches()).toEqual([]);
    expect(result.current.views).toEqual([]);
    expect(nativeRename).not.toHaveBeenCalled();
    expect(nativeDelete).not.toHaveBeenCalled();
  });

  it("persists a shared display order while keeping native routing order authoritative", async () => {
    const a = splitInbox("a", "A", 0), b = splitInbox("b", "B", 1);
    const list = vi.spyOn(mailClient, "listSplitInboxes").mockResolvedValue([a, b]);
    vi.spyOn(mailClient, "reorderSplitInboxes").mockResolvedValue();
    const [saved] = saveSearch({ name: "Search", query: "invoice", accountId: null, inboxOnly: false });
    const id = SAVED_VIEW_PREFIX + saved.id;
    const { result } = renderHook(() => useSplitInboxes());
    await waitFor(() => expect(result.current.loaded).toBe(true));
    list.mockResolvedValue([b, a]);
    await act(async () => { await result.current.reorder(["b", id, "a"]); });
    expect(mailClient.reorderSplitInboxes).toHaveBeenCalledWith(["b", "a"]);
    expect(result.current.views.map((view) => view.id)).toEqual(["b", id, "a"]);
    list.mockResolvedValue([a, b]); // A subsequent imported/synced routing order.
    await act(async () => { await result.current.refresh(); });
    expect(result.current.views.map((view) => view.id)).toEqual(["a", id, "b"]);
  });

  it("reports unreadable saved data without overwriting it or hiding routing splits", async () => {
    localStorage.setItem(SAVED_SEARCHES_KEY, "broken");
    vi.spyOn(mailClient, "listSplitInboxes").mockResolvedValue([splitInbox("a", "A", 0)]);
    const { result } = renderHook(() => useSplitInboxes());
    await waitFor(() => expect(result.current.loaded).toBe(true));
    expect(result.current.error).toBeTruthy();
    expect(result.current.views).toHaveLength(1);
    expect(localStorage.getItem(SAVED_SEARCHES_KEY)).toBe("broken");
  });

  it("marks the catalog loaded even when the first listing fails", async () => {
    const offline = new Error("offline");
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    vi.spyOn(mailClient, "listSplitInboxes").mockRejectedValue(offline);
    const { result } = renderHook(() => useSplitInboxes());

    await waitFor(() => expect(result.current.loaded).toBe(true));
    expect(result.current.splitInboxes).toEqual([]);
    expect(warn).toHaveBeenCalledTimes(1);
    expect(warn).toHaveBeenCalledWith("Split inbox listing failed:", offline);
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
