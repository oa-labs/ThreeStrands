import { registerNoteOwner, NOTES_CHANGED } from "./noteMutations";
import { useCallback, useEffect, useRef, useState } from "react";
import type { Note, NoteRelink, NoteSave } from "./domain";
import { mailClient } from "./data/client";
import { errorMessage } from "./errors";
import { onBeforeExit } from "./beforeExit";
import { clearNoteDraft, noteTargetKey, readNoteDraft, writeNoteDraft, type NoteTarget } from "./noteDrafts";

/**
 * How long typing must pause before a save. Every save reseals and uploads
 * the whole sync snapshot, so the editor doesn't save on every keystroke.
 */
export const NOTE_SAVE_DELAY_MS = 2000;

export type NoteText = { title: string; body: string };

/**
 * - `idle`: nothing waiting, or a save is scheduled.
 * - `stale`: the note changed, went to Trash (`current.trashed`), or was
 *   deleted (`current` null) on another device after this edit began.
 *   Saving waits for the user to choose; `error` says why a choice failed,
 *   with the choice still open.
 */
export type NoteSaveState =
  | { kind: "idle" }
  | { kind: "saving" }
  | { kind: "error"; message: string }
  | { kind: "stale"; current: Note | null; error?: string };

const sameText = (a: NoteText, b: NoteText) => a.title === b.title && a.body === b.body;
// A resolved conflict or a trip through Trash can change a note without a newer save time.
const sameNote = (a: Note | null, b: Note | null) => a === b || (a !== null && b !== null && a.updatedAt === b.updatedAt && a.title === b.title && a.body === b.body && a.trashed === b.trashed);

/**
 * Loads a note into the editor and saves edits: after typing pauses, on
 * `flush` (focus leaving the editor, Escape), when another note opens, when
 * the editor goes away, and when the window closes. Unsaved text is kept as
 * a per-device draft until a save succeeds. Opening a note never writes.
 */
export function useNoteEditor(target: NoteTarget | null, { onSaved, onReplaced, onRestored }: { onSaved?(note: Note, relink?: NoteRelink): void; onReplaced?(note: Note): void; /** A note restored from Trash to keep this device's typing. */ onRestored?(note: Note): void } = {}) {
  const targetKey = target ? noteTargetKey(target) : null;
  const [note, setNote] = useState<Note | null>(null);
  const [text, setText] = useState<NoteText>({ title: "", body: "" });
  const [dirty, setDirty] = useState(false);
  const [loading, setLoading] = useState(false);
  const [missing, setMissing] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [saveState, setSaveState] = useState<NoteSaveState>({ kind: "idle" });

  const targetRef = useRef(target);
  const keyRef = useRef(targetKey);
  const noteRef = useRef<Note | null>(null);
  const textRef = useRef<NoteText>({ title: "", body: "" });
  const dirtyRef = useRef(false);
  const savingRef = useRef<Promise<void> | null>(null);
  const pausedRef = useRef(false);
  const timerRef = useRef<number | null>(null);
  const callbacks = useRef({ onSaved, onReplaced, onRestored });
  callbacks.current = { onSaved, onReplaced, onRestored };

  const clearTimer = () => {
    if (timerRef.current !== null) window.clearTimeout(timerRef.current);
    timerRef.current = null;
  };
  const markDirty = (value: boolean) => { dirtyRef.current = value; setDirty(value); };
  const showNote = (value: Note | null) => { noteRef.current = value; setNote(value); };
  const showText = (value: NoteText) => { textRef.current = value; setText(value); };

  const save = useCallback(async (autosave = false): Promise<void> => {
    clearTimer();
    const current = targetRef.current;
    if (!current || !dirtyRef.current || pausedRef.current) return;
    const key = noteTargetKey(current);
    if (savingRef.current) {
      // A blur/exit must still commit the finished title once the current
      // body save lands. Timer saves can leave newer typing to the next save.
      if (!autosave) {
        await savingRef.current;
        if (keyRef.current === key) await save();
      }
      return;
    }
    // Timers may save the body, but never commit an unconfirmed rename.
    if (autosave && current.kind === "note" && textRef.current.title !== noteRef.current?.title) return;
    const sent = textRef.current;
    const expected = noteRef.current?.updatedAt ?? null;
    if (current.kind === "note" && expected === null) return;
    let finishSave!: () => void;
    savingRef.current = new Promise<void>((resolve) => { finishSave = resolve; });
    setSaveState({ kind: "saving" });
    const here = () => keyRef.current === key;
    try {
      const result: NoteSave = current.kind === "daily"
        ? await mailClient.saveDailyNote(current.date, sent.body, expected)
        : await mailClient.updateNote(current.id, sent.title, sent.body, expected!);
      if (!result.saved) {
        // The draft stays, so reopening the note offers the same choice.
        if (here()) { pausedRef.current = true; setSaveState({ kind: "stale", current: result.note }); }
        return;
      }
      if (result.note && result.relink) callbacks.current.onSaved?.(result.note, result.relink);
      else if (result.note) callbacks.current.onSaved?.(result.note);
      if (!here()) {
        // Typing that arrived during the save stays as a draft on the saved
        // version, so reopening the note picks it up and saves it.
        const draft = readNoteDraft(key);
        if (draft && !sameText(draft, sent)) writeNoteDraft(key, { ...draft, base: result.note?.updatedAt ?? null });
        else clearNoteDraft(key);
        return;
      }
      showNote(result.note);
      const typedSince = !sameText(textRef.current, sent);
      if (typedSince) {
        writeNoteDraft(key, { ...textRef.current, base: result.note?.updatedAt ?? null });
        if (current.kind === "daily" || textRef.current.title === result.note?.title) {
          timerRef.current = window.setTimeout(() => void save(true), NOTE_SAVE_DELAY_MS);
        }
      } else {
        clearNoteDraft(key);
        showText({ title: result.note?.title ?? sent.title, body: result.note?.body ?? sent.body });
        markDirty(false);
      }
      setSaveState({ kind: "idle" });
    } catch (reason) {
      if (here()) setSaveState({ kind: "error", message: errorMessage(reason) });
    } finally {
      savingRef.current = null;
      finishSave();
    }
  }, []);

  useEffect(() => {
    targetRef.current = target;
    keyRef.current = targetKey;
    clearTimer();
    pausedRef.current = false;
    markDirty(false);
    setMissing(false);
    setLoadError(null);
    setSaveState({ kind: "idle" });
    showNote(null);
    showText({ title: "", body: "" });
    if (!target || !targetKey) { setLoading(false); return; }
    let active = true;
    setLoading(true);
    const load = target.kind === "daily" ? mailClient.getDailyNote(target.date) : mailClient.getNote(target.id);
    void load.then((loaded) => {
      if (!active) return;
      showNote(loaded);
      const stored = { title: loaded?.title ?? "", body: loaded?.body ?? "" };
      const draft = readNoteDraft(targetKey);
      if (draft && !sameText(draft, stored)) {
        // Unsaved typing from before a crash or a closed window.
        showText({ title: draft.title, body: draft.body });
        markDirty(true);
        // A note in Trash can't take the typing until the user chooses, even when its text hasn't changed.
        if (draft.base === (loaded?.updatedAt ?? null) && !loaded?.trashed) timerRef.current = window.setTimeout(() => void save(), NOTE_SAVE_DELAY_MS);
        else { pausedRef.current = true; setSaveState({ kind: "stale", current: loaded }); }
        return;
      }
      if (draft) clearNoteDraft(targetKey);
      showText(stored);
      if (target.kind === "note" && !loaded) setMissing(true);
    }, (reason) => { if (active) setLoadError(errorMessage(reason)); }).finally(() => { if (active) setLoading(false); });
    return () => {
      active = false;
      // Saves what the previous note had waiting; refs still name it here.
      void save();
    };
    // `target` is read through its key, so an equal target object doesn't reload.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [targetKey, save]);

  useEffect(() => {
    const flush = () => void save();
    window.addEventListener("pagehide", flush);
    const stopExitSave = onBeforeExit(save);
    return () => { window.removeEventListener("pagehide", flush); stopExitSave(); };
  }, [save]);

  const edit = useCallback((patch: Partial<NoteText>) => {
    const current = targetRef.current;
    if (!current) return;
    const next = { ...textRef.current, ...patch };
    if (sameText(next, textRef.current)) return;
    showText(next);
    markDirty(true);
    writeNoteDraft(noteTargetKey(current), { ...next, base: noteRef.current?.updatedAt ?? null });
    clearTimer();
    if (pausedRef.current) return;
    // A new title saves when the title field is left (`flush`), not while it
    // is typed: renaming rewrites links in other notes, which shouldn't
    // follow every half-typed title.
    if (patch.body === undefined) return;
    timerRef.current = window.setTimeout(() => void save(true), NOTE_SAVE_DELAY_MS);
  }, [save]);

  /** Whether typing is still waiting to be saved, as of now. */
  const unsaved = useCallback(() => dirtyRef.current, []);

  /** Re-reads the note after a sync, unless this device has typing waiting. */
  const refresh = useCallback(async () => {
    const current = targetRef.current;
    if (!current || dirtyRef.current) return;
    const key = noteTargetKey(current);
    const loaded = current.kind === "daily" ? await mailClient.getDailyNote(current.date) : await mailClient.getNote(current.id);
    if (keyRef.current !== key || dirtyRef.current || sameNote(loaded, noteRef.current)) return;
    showNote(loaded);
    showText({ title: loaded?.title ?? "", body: loaded?.body ?? "" });
    setMissing(current.kind === "note" && !loaded);
  }, []);

  /**
   * Replaces the other device's version with this one. A note moved to
   * Trash elsewhere is restored first, then saved with this text.
   */
  const keepMine = useCallback(async () => {
    const current = targetRef.current;
    if (!current || saveState.kind !== "stale") return;
    if (saveState.current?.trashed) {
      const trashed = saveState.current;
      try {
        const restored = await mailClient.restoreNote(trashed.id);
        if (keyRef.current !== noteTargetKey(current)) return;
        callbacks.current.onRestored?.(restored);
        pausedRef.current = false;
        showNote(restored);
        writeNoteDraft(noteTargetKey(current), { ...textRef.current, base: restored.updatedAt });
        setSaveState({ kind: "idle" });
        await save();
      } catch (reason) {
        // Restoring can fail (another note took the title); the choice stays open.
        if (keyRef.current === noteTargetKey(current)) setSaveState({ kind: "stale", current: trashed, error: errorMessage(reason) });
      }
      return;
    }
    pausedRef.current = false;
    if (current.kind === "note" && !saveState.current) {
      // Deleted elsewhere: keep the text as a new note with the same title.
      try {
        const created = await mailClient.createNote(textRef.current.title, textRef.current.body);
        clearNoteDraft(noteTargetKey(current));
        markDirty(false);
        setSaveState({ kind: "idle" });
        callbacks.current.onSaved?.(created);
        callbacks.current.onReplaced?.(created);
      } catch (reason) {
        pausedRef.current = true;
        setSaveState({ kind: "error", message: errorMessage(reason) });
      }
      return;
    }
    showNote(saveState.current);
    writeNoteDraft(noteTargetKey(current), { ...textRef.current, base: saveState.current?.updatedAt ?? null });
    setSaveState({ kind: "idle" });
    await save();
  }, [save, saveState]);

  /** Drops this device's unsaved text and shows the other version. */
  const takeTheirs = useCallback(() => {
    const current = targetRef.current;
    if (!current || saveState.kind !== "stale") return;
    clearNoteDraft(noteTargetKey(current));
    pausedRef.current = false;
    markDirty(false);
    showNote(saveState.current);
    showText({ title: saveState.current?.title ?? "", body: saveState.current?.body ?? "" });
    setMissing(current.kind === "note" && !saveState.current);
    setSaveState({ kind: "idle" });
  }, [saveState]);

  useEffect(() => targetKey ? registerNoteOwner(targetKey, { flush: save, unsaved, refresh, version: () => noteRef.current?.updatedAt ?? null }) : undefined, [targetKey, save, unsaved, refresh]);
  useEffect(() => { const changed = () => { void refresh(); }; window.addEventListener(NOTES_CHANGED, changed); return () => window.removeEventListener(NOTES_CHANGED, changed); }, [refresh]);

  return { note, text, dirty, loading, missing, loadError, saveState, edit, flush: save, unsaved, refresh, keepMine, takeTheirs };
}
