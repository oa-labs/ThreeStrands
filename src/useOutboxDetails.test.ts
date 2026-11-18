import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { summarizeOutbox, type Draft, type OutboxItem } from "./correspondence";
import { mailClient } from "./data/client";
import { useOutboxDetails } from "./useOutboxDetails";

const draft: Draft = { id: "draft", revision: 1, account: "me@example.com", mode: "new", sourceId: null, threadId: null, replyId: null, references: [], to: "you@example.com", cc: "", bcc: "", subject: "Plans", body: "Full body", bodyHtml: "<p>Full body</p>", attachments: [], updatedAt: 1 };
const item: OutboxItem = { id: "send", draft, state: "ready", deadline: 1, error: null };
afterEach(() => vi.restoreAllMocks());

describe("outbox detail loading", () => {
  it("loads only demanded bodies, reuses them through status polls, and releases them on leaving", async () => {
    const get = vi.spyOn(mailClient, "getOutboxItem").mockResolvedValue(item);
    const second = { ...item, id: "another" };
    const { result, rerender } = renderHook(({ ids, items }) => useOutboxDetails(items, ids), { initialProps: { ids: [] as string[], items: [item, second].map(summarizeOutbox) } });
    expect(get).not.toHaveBeenCalled();
    rerender({ ids: [item.id], items: [item, second].map(summarizeOutbox) });
    await waitFor(() => expect(result.current.details[0]?.draft.body).toBe("Full body"));
    expect(get).toHaveBeenCalledExactlyOnceWith(item.id);
    rerender({ ids: [item.id], items: [{ ...item, state: "sent" as const, draft: { ...item.draft, updatedAt: 5 }, sentCopy: { state: "abandoned" as const, error: "No copy" } }, second].map(summarizeOutbox) });
    expect(result.current.details[0]).toMatchObject({ state: "sent", draft: { updatedAt: 5 }, sentCopy: { error: "No copy" } });
    expect(get).toHaveBeenCalledTimes(1);
    rerender({ ids: [], items: [item, second].map(summarizeOutbox) });
    expect(result.current.details).toEqual([]);
    rerender({ ids: [item.id], items: [item, second].map(summarizeOutbox) });
    await waitFor(() => expect(get).toHaveBeenCalledTimes(2));
  });

  it("ignores a late body from the previous selection and reloads a changed revision", async () => {
    let finish!: (item: OutboxItem) => void;
    const second = { ...item, id: "second", draft: { ...draft, id: "second-draft", body: "Second body" } };
    const get = vi.spyOn(mailClient, "getOutboxItem").mockImplementation((id) => id === item.id ? new Promise((resolve) => { finish = resolve; }) : Promise.resolve(second));
    const { result, rerender } = renderHook(({ ids, items }) => useOutboxDetails(items, ids), { initialProps: { ids: [item.id], items: [item, second].map(summarizeOutbox) } });
    rerender({ ids: [second.id], items: [item, second].map(summarizeOutbox) });
    await waitFor(() => expect(result.current.details[0]?.draft.body).toBe("Second body"));
    await act(async () => { finish(item); });
    expect(result.current.details.map((value) => value.id)).toEqual([second.id]);
    const revised = { ...second, draft: { ...second.draft, revision: 2, body: "Revised body" } };
    get.mockResolvedValue(revised);
    rerender({ ids: [second.id], items: [item, revised].map(summarizeOutbox) });
    await waitFor(() => expect(result.current.details[0]?.draft.body).toBe("Revised body"));
    expect(get).toHaveBeenCalledTimes(3);
  });

  it("accepts the requested body revision when its display timestamp changes during the read", async () => {
    vi.spyOn(mailClient, "getOutboxItem").mockResolvedValue({ ...item, draft: { ...draft, updatedAt: 99 } });
    const { result } = renderHook(() => useOutboxDetails([summarizeOutbox(item)], [item.id]));
    await waitFor(() => expect(result.current.details[0]?.draft.body).toBe("Full body"));
    expect(result.current.details[0].draft.updatedAt).toBe(item.draft.updatedAt);
  });

  it("can prime an immediately queued reply without a second native read", async () => {
    const get = vi.spyOn(mailClient, "getOutboxItem");
    const { result, rerender } = renderHook(({ items, ids }) => useOutboxDetails(items, ids), { initialProps: { items: [] as ReturnType<typeof summarizeOutbox>[], ids: [] as string[] } });
    act(() => { result.current.prime(item); });
    rerender({ items: [summarizeOutbox(item)], ids: [item.id] });
    expect(result.current.details[0]?.draft.body).toBe("Full body");
    expect(get).not.toHaveBeenCalled();
  });
});
