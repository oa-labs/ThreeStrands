import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Note } from "../../src/domain";
import type { NoteDraft } from "../../src/noteDrafts";
import { mobile } from "./api";
import { useMobileNote } from "./useMobileNote";

const target = { kind: "note", id: "fixture-note" } as const;
const note: Note = {
  id: target.id, title: "Library", body: "Original", dailyDate: null,
  createdAt: "2026-11-20T00:00:00Z", updatedAt: "2026-11-20T00:00:00Z", trashed: false, trashedAt: null,
};
const open = (value = note) => ({ note: value, document: null, links: null, conflicts: [] });

const drafts = new Map<string, NoteDraft>();
beforeEach(() => {
  drafts.clear();
  vi.spyOn(mobile, "draft").mockImplementation(async key => drafts.get(key) ?? null);
  vi.spyOn(mobile, "writeDraft").mockImplementation(async (key, draft, expected) => {
    if (expected && drafts.get(key) !== expected) return;
    if (draft) drafts.set(key, draft);
    else drafts.delete(key);
  });
});
afterEach(() => vi.restoreAllMocks());

describe("mobile note drafts", () => {
  it("keeps a recoverable draft independently of navigation, even during composition", async () => {
    vi.spyOn(mobile, "note").mockResolvedValue(open());
    const save = vi.spyOn(mobile, "saveNote").mockResolvedValue({ saved: true, note });
    const first = renderHook(() => useMobileNote(target, 0, true));
    await waitFor(() => expect(first.result.current.draftReady).toBe(true));
    expect(first.result.current.note).toEqual(note);
    act(() => first.result.current.edit({ body: "Draft from phone" }));
    first.unmount();
    const second = renderHook(() => useMobileNote(target, 0, true));
    await waitFor(() => expect(second.result.current.body).toBe("Draft from phone"));
    expect(second.result.current.note).toEqual(note);
    expect(drafts.get(target.id)?.base).toBe(note.updatedAt);
    expect(save).not.toHaveBeenCalled();
  });

  it("requires a choice before overwriting a note saved on another device", async () => {
    const remote = { ...note, body: "Desktop edit", updatedAt: "2026-11-20T01:00:00Z" };
    vi.spyOn(mobile, "note").mockResolvedValue(open(remote));
    const save = vi.spyOn(mobile, "saveNote");
    drafts.set(target.id, { title: note.title, body: "Offline edit", base: note.updatedAt });
    const { result } = renderHook(() => useMobileNote(target, 0, false));
    await waitFor(() => expect(result.current.stale).toBe(true));
    expect(result.current.body).toBe("Offline edit");
    await act(async () => { expect(await result.current.save()).toBe(false); });
    expect(save).not.toHaveBeenCalled();
    expect(drafts.get(target.id)?.body).toBe("Offline edit");
  });
});
