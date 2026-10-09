import { act, cleanup, fireEvent, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mailClient } from "./data/client";
import type { Thread } from "./domain";
import { useThreadSelection } from "./useThreadSelection";

describe("useThreadSelection", () => {
  let threads: Thread[];
  beforeEach(async () => { threads = (await mailClient.listThreads()).slice(0, 3); });
  afterEach(cleanup);

  function setup() {
    const contextOpenedThreadRef = { current: "context-thread" as string | null };
    const setSelectedId = vi.fn();
    const hook = renderHook((props: { threads: Thread[]; query: string }) => useThreadSelection({
      ...props,
      selectedId: threads[0]!.id,
      setSelectedId,
      includeArchived: false,
      contextOpenedThreadRef,
    }), { initialProps: { threads, query: "" } });
    return { ...hook, contextOpenedThreadRef, setSelectedId };
  }

  it("uses the latest visible order for range selection while keeping gesture callbacks stable", () => {
    const { result, rerender } = setup();
    const applyGesture = result.current.applyThreadSelectionGesture;
    rerender({ threads: [threads[0]!, threads[2]!], query: "" });
    expect(result.current.applyThreadSelectionGesture).toBe(applyGesture);

    act(() => applyGesture(threads[2]!.id, "range"));
    expect(result.current.checkedIds).toEqual(new Set([threads[0]!.id, threads[2]!.id]));
  });

  it("prunes removed rows and resets checked rows when the search changes", () => {
    const { result, rerender } = setup();
    act(() => result.current.setCheckedIds(new Set(threads.map((thread) => thread.id))));
    rerender({ threads: threads.slice(1), query: "" });
    expect(result.current.checkedIds).toEqual(new Set(threads.slice(1).map((thread) => thread.id)));

    rerender({ threads: threads.slice(1), query: "new search" });
    expect(result.current.checkedIds.size).toBe(0);
  });

  it("clears context-jump retention and checked rows when a conversation is selected", () => {
    const { result, contextOpenedThreadRef, setSelectedId } = setup();
    act(() => result.current.toggleChecked(threads[0]!.id));
    act(() => result.current.selectThread(threads[1]!.id));
    expect(contextOpenedThreadRef.current).toBeNull();
    expect(result.current.checkedIds.size).toBe(0);
    expect(setSelectedId).toHaveBeenCalledWith(threads[1]!.id);
  });

  it("clears both checked rows and message filters with Escape", () => {
    const { result } = setup();
    act(() => {
      result.current.toggleChecked(threads[0]!.id);
      result.current.toggleMessageFilter("unread");
    });
    fireEvent.keyDown(window, { key: "Escape" });
    expect(result.current.checkedIds.size).toBe(0);
    expect(result.current.activeMessageFilters.size).toBe(0);
  });
});
