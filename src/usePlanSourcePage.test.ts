import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { mailClient } from "./data/client";
import type { PlanCandidate, PlanSourcePage } from "./domain";
import { usePlanSourcePage } from "./usePlanSourcePage";

const candidate = (key: string): PlanCandidate => ({ key, title: key, reason: "starred", detail: null, accountId: "one", due: null, taskId: null, threadId: key, contactId: null, goal: null, plannedDays: [], openTasks: null, planned: false, action: { kind: "taskForm", threadId: key, accountId: "one" } });
afterEach(() => vi.restoreAllMocks());
describe("usePlanSourcePage refresh", () => {
  it("removes a settled suggestion without resetting pagination or mutating a later account's list", async () => {
    vi.spyOn(mailClient, "planSources").mockImplementation(async (_date, _source, account) => ({ candidates: [candidate(account!)], next: "next" }));
    const view = renderHook(({ account }) => usePlanSourcePage("2026-11-17", "starredMail", account, "1", true), { initialProps: { account: "one" } });
    await waitFor(() => expect(view.result.current.candidates[0].key).toBe("one"));
    const removeOld = view.result.current.remove;
    act(() => removeOld("one"));
    expect(view.result.current.candidates).toEqual([]);
    expect(view.result.current.next).toBe("next");
    view.rerender({ account: "two" });
    await waitFor(() => expect(view.result.current.candidates[0].key).toBe("two"));
    act(() => removeOld("two"));
    expect(view.result.current.candidates[0].key).toBe("two");
  });

  it("keeps cards and every expanded page while refreshing, then replaces them with fresh results", async () => {
    const source = vi.spyOn(mailClient, "planSources").mockResolvedValueOnce({ candidates: [candidate("first")], next: "next" }).mockResolvedValueOnce({ candidates: [candidate("second")], next: null });
    const view = renderHook(({ revision }) => usePlanSourcePage("2026-11-17", "starredMail", "one", revision, true), { initialProps: { revision: "1" } });
    await waitFor(() => expect(view.result.current.candidates).toHaveLength(1));
    await act(() => view.result.current.loadMore());
    expect(view.result.current.candidates.map((row) => row.title)).toEqual(["first", "second"]);
    let finish!: (page: PlanSourcePage) => void;
    source.mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; })).mockResolvedValueOnce({ candidates: [candidate("second-fresh")], next: null });
    view.rerender({ revision: "2" });
    expect(view.result.current.loading).toBe(true);
    expect(view.result.current.candidates.map((row) => row.title)).toEqual(["first", "second"]);
    await act(async () => finish({ candidates: [candidate("first-fresh")], next: "fresh-next" }));
    await waitFor(() => expect(view.result.current.candidates.map((row) => row.title)).toEqual(["first-fresh", "second-fresh"]));
    expect(source).toHaveBeenLastCalledWith("2026-11-17", "starredMail", "one", "fresh-next");
  });

  it("keeps previously read cards when a refresh fails and reports the failure", async () => {
    const source = vi.spyOn(mailClient, "planSources").mockResolvedValue({ candidates: [candidate("keep")], next: null });
    const view = renderHook(({ revision }) => usePlanSourcePage("2026-11-17", "notes", null, revision, true), { initialProps: { revision: "1" } });
    await waitFor(() => expect(view.result.current.candidates).toHaveLength(1));
    source.mockRejectedValue(new Error("Locked")); view.rerender({ revision: "2" });
    await waitFor(() => expect(view.result.current.error).toBe("Locked"));
    expect(view.result.current.candidates[0].title).toBe("keep");
  });

  it("does not retain another account's cards or accept its late response", async () => {
    let finish!: (page: PlanSourcePage) => void;
    const source = vi.spyOn(mailClient, "planSources").mockResolvedValueOnce({ candidates: [candidate("one")], next: null });
    const view = renderHook(({ account, revision }) => usePlanSourcePage("2026-11-17", "starredMail", account, revision, true), { initialProps: { account: "one", revision: "1" } });
    await waitFor(() => expect(view.result.current.candidates).toHaveLength(1));
    source.mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; }));
    view.rerender({ account: "one", revision: "2" });
    source.mockResolvedValueOnce({ candidates: [candidate("two")], next: null });
    view.rerender({ account: "two", revision: "2" });
    expect(view.result.current.candidates).toEqual([]);
    await waitFor(() => expect(view.result.current.candidates[0].title).toBe("two"));
    await act(async () => finish({ candidates: [candidate("late-one")], next: null }));
    expect(view.result.current.candidates[0].title).toBe("two");
  });
});
