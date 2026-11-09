import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mailClient } from "./data/client";
import { readNoteDraft, writeNoteDraft, type NoteTarget } from "./noteDrafts";
import { runBeforeExitTasks } from "./beforeExit";
import { NOTE_SAVE_DELAY_MS, useNoteEditor } from "./useNoteEditor";

let counter = 0;
const uniqueTitle = (label: string) => `${label} ${++counter}`;
const settle = () => act(async () => { await vi.advanceTimersByTimeAsync(0); });
const wait = (ms: number) => act(async () => { await vi.advanceTimersByTimeAsync(ms); });

function open(target: NoteTarget | null, options?: Parameters<typeof useNoteEditor>[1]) {
  return renderHook(({ current }) => useNoteEditor(current, options), { initialProps: { current: target } });
}

describe("useNoteEditor", () => {
  beforeEach(() => { vi.useFakeTimers(); });
  afterEach(() => { vi.useRealTimers(); vi.restoreAllMocks(); localStorage.clear(); });

  it("opens a note without writing anything", async () => {
    const note = await mailClient.createNote(uniqueTitle("Read only"), "Body");
    const update = vi.spyOn(mailClient, "updateNote");
    const daily = vi.spyOn(mailClient, "saveDailyNote");
    const { result, unmount } = open({ kind: "note", id: note.id });
    await settle();
    expect(result.current.text).toEqual({ title: note.title, body: "Body" });
    await wait(NOTE_SAVE_DELAY_MS * 3);
    await act(async () => { await result.current.flush(); });
    unmount();
    await settle();
    expect(update).not.toHaveBeenCalled();
    expect(daily).not.toHaveBeenCalled();
  });

  it("saves once typing pauses, keeping a draft until the save lands", async () => {
    const note = await mailClient.createNote(uniqueTitle("Typing"), "");
    const onSaved = vi.fn();
    const update = vi.spyOn(mailClient, "updateNote");
    const { result } = open({ kind: "note", id: note.id }, { onSaved });
    await settle();
    act(() => result.current.edit({ body: "a" }));
    await wait(NOTE_SAVE_DELAY_MS - 1);
    act(() => result.current.edit({ body: "ab" }));
    await wait(NOTE_SAVE_DELAY_MS - 1);
    expect(update).not.toHaveBeenCalled();
    expect(result.current.dirty).toBe(true);
    expect(readNoteDraft(note.id)).toEqual({ title: note.title, body: "ab", base: note.updatedAt });
    await wait(1);
    expect(update).toHaveBeenCalledTimes(1);
    expect(update).toHaveBeenCalledWith(note.id, note.title, "ab", note.updatedAt);
    expect(result.current.dirty).toBe(false);
    expect(result.current.saveState).toEqual({ kind: "idle" });
    expect(readNoteDraft(note.id)).toBeNull();
    expect(onSaved).toHaveBeenCalledWith(expect.objectContaining({ body: "ab" }));
    expect((await mailClient.getNote(note.id))?.body).toBe("ab");
  });

  it("saves right away when another note opens, the window hides, the app quits, or the editor closes", async () => {
    const first = await mailClient.createNote(uniqueTitle("First"), "");
    const second = await mailClient.createNote(uniqueTitle("Second"), "");
    const { result, rerender, unmount } = open({ kind: "note", id: first.id });
    await settle();
    act(() => result.current.edit({ body: "switching" }));
    rerender({ current: { kind: "note", id: second.id } });
    await settle();
    expect((await mailClient.getNote(first.id))?.body).toBe("switching");
    expect(result.current.text.body).toBe("");

    act(() => result.current.edit({ body: "hidden" }));
    act(() => { window.dispatchEvent(new Event("pagehide")); });
    await settle();
    expect((await mailClient.getNote(second.id))?.body).toBe("hidden");

    act(() => result.current.edit({ body: "quitting" }));
    await act(async () => { await runBeforeExitTasks(); });
    expect((await mailClient.getNote(second.id))?.body).toBe("quitting");

    act(() => result.current.edit({ body: "closing" }));
    unmount();
    await settle();
    expect((await mailClient.getNote(second.id))?.body).toBe("closing");
  });

  it("doesn't create a day's note until something is typed", async () => {
    const date = "2031-01-02";
    const { result } = open({ kind: "daily", date });
    await settle();
    act(() => result.current.edit({ body: "x" }));
    act(() => result.current.edit({ body: "" }));
    await wait(NOTE_SAVE_DELAY_MS);
    expect(await mailClient.getDailyNote(date)).toBeNull();
    act(() => result.current.edit({ body: "Standup" }));
    await wait(NOTE_SAVE_DELAY_MS);
    expect(result.current.note).toMatchObject({ id: `daily:${date}`, body: "Standup" });
    act(() => result.current.edit({ body: "Standup notes" }));
    await wait(NOTE_SAVE_DELAY_MS);
    expect((await mailClient.getDailyNote(date))?.body).toBe("Standup notes");
  });

  it("stops and asks when the note changed elsewhere, then keeps either version", async () => {
    const note = await mailClient.createNote(uniqueTitle("Shared"), "v1");
    const { result } = open({ kind: "note", id: note.id });
    await settle();
    act(() => result.current.edit({ body: "mine" }));
    const theirs = await mailClient.updateNote(note.id, note.title, "theirs", note.updatedAt);
    await act(async () => { await result.current.flush(); });
    expect(result.current.saveState).toEqual({ kind: "stale", current: theirs.note });
    expect(result.current.text.body).toBe("mine");
    expect((await mailClient.getNote(note.id))?.body).toBe("theirs");
    // Saving waits for the choice, even after more typing.
    act(() => result.current.edit({ body: "mine, more" }));
    await wait(NOTE_SAVE_DELAY_MS * 2);
    expect((await mailClient.getNote(note.id))?.body).toBe("theirs");

    await act(async () => { await result.current.keepMine(); });
    expect((await mailClient.getNote(note.id))?.body).toBe("mine, more");
    expect(result.current.saveState).toEqual({ kind: "idle" });

    act(() => result.current.edit({ body: "mine again" }));
    const latest = (await mailClient.getNote(note.id))!;
    await mailClient.updateNote(note.id, note.title, "theirs again", latest.updatedAt);
    await act(async () => { await result.current.flush(); });
    act(() => result.current.takeTheirs());
    expect(result.current.text.body).toBe("theirs again");
    expect(result.current.dirty).toBe(false);
    expect(readNoteDraft(note.id)).toBeNull();
  });

  it("asks when the note went to Trash elsewhere, then restores it to keep the typing or shows it in Trash", async () => {
    const note = await mailClient.createNote(uniqueTitle("Trashed elsewhere"), "v1");
    const onRestored = vi.fn();
    const { result } = open({ kind: "note", id: note.id }, { onRestored });
    await settle();
    act(() => result.current.edit({ body: "mine" }));
    // Another device trashes it; trashing changes no text, so the save time stays.
    const trashed = await mailClient.trashNote(note.id);
    expect(trashed.updatedAt).toBe(note.updatedAt);
    await act(async () => { await result.current.flush(); });
    expect(result.current.saveState).toEqual({ kind: "stale", current: trashed });
    expect(result.current.text.body).toBe("mine");

    await act(async () => { await result.current.keepMine(); });
    expect(onRestored).toHaveBeenCalledWith(expect.objectContaining({ id: note.id, trashed: false }));
    expect(await mailClient.getNote(note.id)).toMatchObject({ trashed: false, body: "mine" });
    expect(result.current.saveState).toEqual({ kind: "idle" });
    expect(readNoteDraft(note.id)).toBeNull();

    // Discarding instead shows the note as it is now, in Trash.
    act(() => result.current.edit({ body: "mine again" }));
    await mailClient.trashNote(note.id);
    await act(async () => { await result.current.flush(); });
    act(() => result.current.takeTheirs());
    expect(result.current.note).toMatchObject({ trashed: true, body: "mine" });
    expect(result.current.text.body).toBe("mine");
    expect(result.current.dirty).toBe(false);
  });

  it("keeps the choice open when restoring from Trash fails", async () => {
    const note = await mailClient.createNote(uniqueTitle("Taken title"), "v1");
    const { result } = open({ kind: "note", id: note.id });
    await settle();
    act(() => result.current.edit({ body: "mine" }));
    const trashed = await mailClient.trashNote(note.id);
    await act(async () => { await result.current.flush(); });
    vi.spyOn(mailClient, "restoreNote").mockRejectedValue(new Error("A note named “Taken title” already exists."));
    const update = vi.spyOn(mailClient, "updateNote");
    await act(async () => { await result.current.keepMine(); });
    expect(result.current.saveState).toEqual({ kind: "stale", current: trashed, error: "A note named “Taken title” already exists." });
    expect(update).not.toHaveBeenCalled();
    expect(result.current.text.body).toBe("mine");
    await wait(NOTE_SAVE_DELAY_MS * 2);
    expect(update).not.toHaveBeenCalled();
  });

  it("after a restart, asks about typing in a note that went to Trash instead of retrying the save", async () => {
    const note = await mailClient.createNote(uniqueTitle("Trashed while closed"), "saved");
    writeNoteDraft(note.id, { title: note.title, body: "typed before quitting", base: note.updatedAt });
    const trashed = await mailClient.trashNote(note.id);
    const update = vi.spyOn(mailClient, "updateNote");
    const { result } = open({ kind: "note", id: note.id });
    await settle();
    expect(result.current.text.body).toBe("typed before quitting");
    expect(result.current.saveState).toEqual({ kind: "stale", current: trashed });
    await wait(NOTE_SAVE_DELAY_MS * 2);
    expect(update).not.toHaveBeenCalled();
  });

  it("asks when today's note went to Trash elsewhere", async () => {
    const date = "2026-03-14";
    const saved = (await mailClient.saveDailyNote(date, "morning", null)).note!;
    const { result } = open({ kind: "daily", date });
    await settle();
    act(() => result.current.edit({ body: "morning, more" }));
    const trashed = await mailClient.trashNote(saved.id);
    await act(async () => { await result.current.flush(); });
    expect(result.current.saveState).toEqual({ kind: "stale", current: trashed });
    await act(async () => { await result.current.keepMine(); });
    expect(await mailClient.getDailyNote(date)).toMatchObject({ trashed: false, body: "morning, more" });
  });

  it("restores unsaved text after a restart, asking first if the note changed since", async () => {
    const note = await mailClient.createNote(uniqueTitle("Restart"), "saved");
    writeNoteDraft(note.id, { title: note.title, body: "typed before quitting", base: note.updatedAt });
    const restored = open({ kind: "note", id: note.id });
    await settle();
    expect(restored.result.current.text.body).toBe("typed before quitting");
    expect(restored.result.current.dirty).toBe(true);
    await wait(NOTE_SAVE_DELAY_MS);
    expect((await mailClient.getNote(note.id))?.body).toBe("typed before quitting");
    restored.unmount();

    writeNoteDraft(note.id, { title: note.title, body: "from an old version", base: note.updatedAt });
    const asked = open({ kind: "note", id: note.id });
    await settle();
    expect(asked.result.current.text.body).toBe("from an old version");
    expect(asked.result.current.saveState).toMatchObject({ kind: "stale", current: { body: "typed before quitting" } });
  });

  it("keeps text typed into a note deleted elsewhere as a new note", async () => {
    writeNoteDraft("gone", { title: uniqueTitle("Gone"), body: "rescued", base: "v1" });
    vi.spyOn(mailClient, "getNote").mockResolvedValue(null);
    const onReplaced = vi.fn();
    const { result } = open({ kind: "note", id: "gone" }, { onReplaced });
    await settle();
    expect(result.current.saveState).toEqual({ kind: "stale", current: null });
    await act(async () => { await result.current.keepMine(); });
    expect(onReplaced).toHaveBeenCalledWith(expect.objectContaining({ body: "rescued" }));
    expect(readNoteDraft("gone")).toBeNull();
  });

  it("shows a refused save and keeps the text", async () => {
    const note = await mailClient.createNote(uniqueTitle("Refused"), "");
    vi.spyOn(mailClient, "updateNote").mockRejectedValueOnce("This note is too long to sync.");
    const { result } = open({ kind: "note", id: note.id });
    await settle();
    act(() => result.current.edit({ body: "long" }));
    await wait(NOTE_SAVE_DELAY_MS);
    expect(result.current.saveState).toEqual({ kind: "error", message: "This note is too long to sync." });
    expect(result.current.dirty).toBe(true);
    expect(readNoteDraft(note.id)?.body).toBe("long");
  });

  it("picks up a synced change only while nothing is waiting to save", async () => {
    const note = await mailClient.createNote(uniqueTitle("Synced"), "v1");
    const { result } = open({ kind: "note", id: note.id });
    await settle();
    await mailClient.updateNote(note.id, note.title, "v2", note.updatedAt);
    await act(async () => { await result.current.refresh(); });
    expect(result.current.text.body).toBe("v2");
    act(() => result.current.edit({ body: "typing" }));
    const latest = (await mailClient.getNote(note.id))!;
    await mailClient.updateNote(note.id, note.title, "v3", latest.updatedAt);
    await act(async () => { await result.current.refresh(); });
    expect(result.current.text.body).toBe("typing");
  });
  it("picks up a change that keeps the save time, such as a trip to Trash", async () => {
    const note = await mailClient.createNote(uniqueTitle("Trashed"), "v1");
    const { result } = open({ kind: "note", id: note.id });
    await settle();
    await mailClient.trashNote(note.id);
    expect((await mailClient.getNote(note.id))?.updatedAt).toBe(note.updatedAt);
    await act(async () => { await result.current.refresh(); });
    expect(result.current.note?.trashed).toBe(true);
  });

  it("says whether typing is still waiting after a save attempt", async () => {
    const note = await mailClient.createNote(uniqueTitle("Pending"), "v1");
    const { result } = open({ kind: "note", id: note.id });
    await settle();
    act(() => result.current.edit({ body: "v2" }));
    expect(result.current.unsaved()).toBe(true);
    await act(async () => { await result.current.flush(); });
    expect(result.current.unsaved()).toBe(false);
  });
  it("cancels a pending body save while a title is being typed, then renames and relinks on flush", async () => {
    const target = await mailClient.createNote(uniqueTitle("Pending body"), "original body");
    const linking = await mailClient.createNote(uniqueTitle("Backlink"), `See [[${target.title}]]`);
    const update = vi.spyOn(mailClient, "updateNote");
    const { result } = open({ kind: "note", id: target.id });
    await settle();
    act(() => result.current.edit({ body: "edited body" }));
    await wait(NOTE_SAVE_DELAY_MS - 1);
    act(() => result.current.edit({ title: "Half-typed" }));
    await wait(NOTE_SAVE_DELAY_MS * 2);
    expect(update).not.toHaveBeenCalled();
    expect((await mailClient.getNote(target.id))?.title).toBe(target.title);
    expect((await mailClient.getNote(linking.id))?.body).toBe(`See [[${target.title}]]`);
    expect(readNoteDraft(target.id)).toMatchObject({ title: "Half-typed", body: "edited body" });
    // Further body typing must not smuggle the pending title into a new timer.
    act(() => result.current.edit({ body: "more body text" }));
    await wait(NOTE_SAVE_DELAY_MS);
    expect(update).not.toHaveBeenCalled();
    expect(readNoteDraft(target.id)).toMatchObject({ title: "Half-typed", body: "more body text" });
    const finished = uniqueTitle("Finished rename");
    act(() => result.current.edit({ title: finished }));
    await act(async () => { await result.current.flush(); });
    expect(update).toHaveBeenCalledExactlyOnceWith(target.id, finished, "more body text", target.updatedAt);
    expect((await mailClient.getNote(linking.id))?.body).toBe(`See [[${finished}]]`);
    expect(result.current.dirty).toBe(false);
  });

  it.each([false, true])("keeps title edits out of in-flight body autosaves (flush while saving: %s)", async (flushWhileSaving) => {
    const target = await mailClient.createNote(uniqueTitle("In flight"), "original body");
    const linking = await mailClient.createNote(uniqueTitle("In flight backlink"), `See [[${target.title}]]`);
    let release!: () => void;
    const gate = new Promise<void>((resolve) => { release = resolve; });
    const realUpdate = mailClient.updateNote.bind(mailClient);
    const update = vi.spyOn(mailClient, "updateNote").mockImplementationOnce(async (...args) => {
      await gate;
      return realUpdate(...args);
    });
    const { result } = open({ kind: "note", id: target.id });
    await settle();
    act(() => result.current.edit({ body: "edited body" }));
    await wait(NOTE_SAVE_DELAY_MS);
    expect(result.current.saveState.kind).toBe("saving");
    const finished = uniqueTitle("Complete title");
    act(() => result.current.edit({ title: finished }));
    let flushed: Promise<void> | undefined;
    if (flushWhileSaving) act(() => { flushed = result.current.flush(); });
    await act(async () => { release(); await gate; });
    await settle();
    if (!flushWhileSaving) {
      await wait(NOTE_SAVE_DELAY_MS * 2);
      expect(update).toHaveBeenCalledTimes(1);
      expect(result.current.dirty).toBe(true);
      expect((await mailClient.getNote(target.id))?.title).toBe(target.title);
      expect((await mailClient.getNote(linking.id))?.body).toBe(`See [[${target.title}]]`);
      await act(async () => { await result.current.flush(); });
    } else {
      await act(async () => { await flushed; });
    }
    expect(update).toHaveBeenCalledTimes(2);
    expect(update.mock.calls[0]).toEqual([target.id, target.title, "edited body", target.updatedAt]);
    expect(update.mock.calls[1]?.slice(0, 3)).toEqual([target.id, finished, "edited body"]);
    expect((await mailClient.getNote(linking.id))?.body).toBe(`See [[${finished}]]`);
    expect(result.current.dirty).toBe(false);
  });

  it("saves a new title when asked to, not while it is typed, and reports what the rename did", async () => {
    const target = await mailClient.createNote(uniqueTitle("Target"), "");
    const linking = await mailClient.createNote(uniqueTitle("Linking"), `See [[${target.title}]]`);
    const update = vi.spyOn(mailClient, "updateNote");
    const onSaved = vi.fn();
    const { result } = open({ kind: "note", id: target.id }, { onSaved });
    await settle();
    const renamed = uniqueTitle("Renamed");
    act(() => result.current.edit({ title: renamed }));
    await wait(NOTE_SAVE_DELAY_MS * 2);
    expect(update).not.toHaveBeenCalled();
    expect(readNoteDraft(target.id)?.title).toBe(renamed);
    await act(async () => { await result.current.flush(); });
    expect(update).toHaveBeenCalledWith(target.id, renamed, "", target.updatedAt);
    expect(onSaved).toHaveBeenLastCalledWith(expect.objectContaining({ title: renamed }), { updated: 1, keptOldLink: [] });
    expect((await mailClient.getNote(linking.id))?.body).toBe(`See [[${renamed}]]`);
  });
});
