import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { mailClient } from "./data/client";
import type { NoteTaskRecord } from "./noteEntities";
import { NOTES_CHANGED } from "./noteMutations";
import { useTodayCompletedNotes } from "./useTodayCompletedNotes";

const record = (id: string, overrides: Partial<NoteTaskRecord> = {}): NoteTaskRecord => ({
  id: `note:n:${id}`, markerId: id, noteId: "n", noteTitle: "Notes", title: id, checked: true,
  completedAt: new Date(2026, 10, 17, 10).toISOString(), updatedAt: "r1", conflicted: false, ...overrides,
});
afterEach(() => vi.restoreAllMocks());

describe("useTodayCompletedNotes", () => {
  it("reads saved completions from the local day and excludes open, conflicted, and older tasks", async () => {
    vi.spyOn(mailClient, "listNoteTasks").mockResolvedValue([
      record("midnight", { completedAt: new Date(2026, 10, 17).toISOString() }), record("done"),
      record("yesterday", { completedAt: new Date(2026, 10, 16, 23, 59).toISOString() }),
      record("tomorrow", { completedAt: new Date(2026, 10, 18).toISOString() }),
      record("open", { checked: false }), record("conflicted", { conflicted: true }), record("unknown", { completedAt: null }),
    ]);
    const view = renderHook(() => useTodayCompletedNotes("2026-11-17", "1"));
    await waitFor(() => expect(view.result.current.tasks.map((task) => task.title)).toEqual(["midnight", "done"]));
  });

  it("keeps history across a failed refresh and updates when a note task is reopened", async () => {
    const read = vi.spyOn(mailClient, "listNoteTasks").mockResolvedValue([record("done")]);
    const view = renderHook(({ revision }) => useTodayCompletedNotes("2026-11-17", revision), { initialProps: { revision: "1" } });
    await waitFor(() => expect(view.result.current.tasks).toHaveLength(1));
    read.mockRejectedValue(new Error("Read failed")); view.rerender({ revision: "2" });
    await waitFor(() => expect(view.result.current.error).toBe("Read failed"));
    expect(view.result.current.tasks).toHaveLength(1);
    read.mockResolvedValue([record("done", { checked: false, completedAt: null })]);
    act(() => window.dispatchEvent(new CustomEvent(NOTES_CHANGED)));
    await waitFor(() => expect(view.result.current.tasks).toEqual([]));
    expect(view.result.current.error).toBeNull();
  });

  it("does not show yesterday's history or accept its late response after the day changes", async () => {
    let finish!: (tasks: NoteTaskRecord[]) => void;
    const read = vi.spyOn(mailClient, "listNoteTasks").mockResolvedValueOnce([record("yesterday")]);
    const view = renderHook(({ date, revision }) => useTodayCompletedNotes(date, revision), { initialProps: { date: "2026-11-17", revision: "1" } });
    await waitFor(() => expect(view.result.current.tasks).toHaveLength(1));
    read.mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; }));
    view.rerender({ date: "2026-11-17", revision: "2" });
    read.mockResolvedValueOnce([record("new day", { completedAt: new Date(2026, 10, 18, 10).toISOString() })]);
    view.rerender({ date: "2026-11-18", revision: "2" });
    expect(view.result.current.tasks).toEqual([]);
    await waitFor(() => expect(view.result.current.tasks[0].title).toBe("new day"));
    await act(async () => finish([record("late")]));
    expect(view.result.current.tasks[0].title).toBe("new day");
  });
});
