import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mailClient } from "./data/client";
import type { CreatedScheduleEvent, DoneNote } from "./domain";
import { NOTES_CHANGED } from "./noteMutations";
import { DONE_NOTES_LIMIT, useDoneOnDay } from "./useDoneOnDay";

afterEach(() => vi.restoreAllMocks());
beforeEach(() => { vi.spyOn(mailClient, "listEventsCreatedBetween").mockResolvedValue({ events: [], errors: [] }); });
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

  it("stays settled while a background refresh reads the same day", async () => {
    vi.spyOn(mailClient, "listSentBetween").mockResolvedValue([]);
    const read = vi.spyOn(mailClient, "listNotesDoneBetween").mockResolvedValue([]);
    const view = renderHook(({ selected, revision }) => useDoneOnDay(selected, null, revision, true), {
      initialProps: { selected: day, revision: "1" },
    });
    expect(view.result.current.loading).toBe(true);
    await waitFor(() => expect(view.result.current.loading).toBe(false));
    let finish!: (items: DoneNote[]) => void;
    read.mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; }));
    view.rerender({ selected: day, revision: "2" });
    expect(read).toHaveBeenCalledTimes(2);
    expect(view.result.current.loading).toBe(false);
    await act(async () => finish([edited]));
    await waitFor(() => expect(view.result.current.notes).toEqual([edited]));
    read.mockImplementationOnce(() => new Promise(() => {}));
    act(() => window.dispatchEvent(new CustomEvent(NOTES_CHANGED)));
    expect(read).toHaveBeenCalledTimes(3);
    expect(view.result.current.loading).toBe(false);
    expect(view.result.current.notes).toEqual([edited]);
    read.mockImplementationOnce(() => new Promise(() => {}));
    view.rerender({ selected: new Date(2026, 10, 16, 12), revision: "2" });
    expect(view.result.current.loading).toBe(true);
    expect(view.result.current.notes).toEqual([]);
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

  it("reads the meetings created on the local day and keeps the other sources when it fails", async () => {
    const meeting = { id: "primary:m", uid: "m@google.com", accountId: "me@example.com", title: "Kickoff", start: "2026-12-20T15:00:00Z", end: "2026-12-20T16:00:00Z", allDay: false, attendees: ["ana@example.com"], created: new Date(2026, 10, 17, 9).toISOString() } as CreatedScheduleEvent;
    vi.spyOn(mailClient, "listSentBetween").mockResolvedValue([]);
    vi.spyOn(mailClient, "listNotesDoneBetween").mockResolvedValue([edited]);
    const read = vi.mocked(mailClient.listEventsCreatedBetween).mockResolvedValue({ events: [meeting], errors: [] });
    const view = renderHook(() => useDoneOnDay(day, null, "1", true));
    await waitFor(() => expect(view.result.current.meetings).toEqual([meeting]));
    expect(read).toHaveBeenCalledWith(new Date(2026, 10, 17).toISOString(), new Date(2026, 10, 18).toISOString(), Intl.DateTimeFormat().resolvedOptions().timeZone);
    read.mockRejectedValue(new Error("Calendar is offline"));
    view.rerender();
    const failing = renderHook(() => useDoneOnDay(day, null, "2", true));
    await waitFor(() => expect(failing.result.current.errors.meetings).toBe("Calendar is offline"));
    expect(failing.result.current.notes).toEqual([edited]);
    expect(failing.result.current.errors.notes).toBeUndefined();
    expect(failing.result.current.errors.emails).toBeUndefined();
  });

  it("reports calendar errors only when no meeting came back", async () => {
    vi.spyOn(mailClient, "listSentBetween").mockResolvedValue([]);
    vi.spyOn(mailClient, "listNotesDoneBetween").mockResolvedValue([]);
    vi.mocked(mailClient.listEventsCreatedBetween).mockResolvedValue({ events: [], errors: ["a@example.com: denied"] });
    const view = renderHook(() => useDoneOnDay(day, null, "1", true));
    await waitFor(() => expect(view.result.current.errors.meetings).toBe("a@example.com: denied"));
  });
});
