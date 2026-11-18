import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { mailClient } from "./data/client";
import type { DoneNote } from "./domain";
import { NOTES_CHANGED } from "./noteMutations";
import { DONE_NOTES_LIMIT, useDoneOnDay } from "./useDoneOnDay";

afterEach(() => vi.restoreAllMocks());
const day = new Date(2026, 10, 17, 12);
const edited: DoneNote = { id: "star", title: "Project", preview: "Progress", at: new Date(2026, 10, 17, 10).toISOString(), edited: true };

describe("useDoneOnDay", () => {
  it("reads saved note activity for the local day and refreshes it after note changes", async () => {
    vi.spyOn(mailClient, "listSentBetween").mockResolvedValue([]);
    const read = vi.spyOn(mailClient, "listNotesDoneBetween").mockResolvedValue([edited]);
    const view = renderHook(() => useDoneOnDay(day, null, "1", true));
    await waitFor(() => expect(view.result.current.notes).toEqual([edited]));
    expect(read).toHaveBeenCalledWith(new Date(2026, 10, 17).toISOString(), new Date(2026, 10, 18).toISOString(), DONE_NOTES_LIMIT);
    read.mockResolvedValue([]);
    act(() => window.dispatchEvent(new CustomEvent(NOTES_CHANGED)));
    await waitFor(() => expect(view.result.current.notes).toEqual([]));
  });

  it("keeps note activity on a failed refresh and ignores a late response for another day", async () => {
    vi.spyOn(mailClient, "listSentBetween").mockResolvedValue([]);
    const read = vi.spyOn(mailClient, "listNotesDoneBetween").mockResolvedValue([edited]);
    const view = renderHook(({ selected, revision }) => useDoneOnDay(selected, null, revision, true), {
      initialProps: { selected: day, revision: "1" },
    });
    await waitFor(() => expect(view.result.current.notes).toEqual([edited]));
    read.mockRejectedValueOnce(new Error("Offline"));
    view.rerender({ selected: day, revision: "2" });
    await waitFor(() => expect(view.result.current.errors.notes).toBe("Offline"));
    expect(view.result.current.notes).toEqual([edited]);
    let finish!: (items: DoneNote[]) => void;
    read.mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; }));
    view.rerender({ selected: day, revision: "3" });
    read.mockResolvedValue([]);
    view.rerender({ selected: new Date(2026, 10, 16, 12), revision: "3" });
    expect(view.result.current.notes).toEqual([]);
    await waitFor(() => expect(view.result.current.loading).toBe(false));
    await act(async () => finish([edited]));
    expect(view.result.current.notes).toEqual([]);
    expect(view.result.current.errors.notes).toBeUndefined();
  });
});
