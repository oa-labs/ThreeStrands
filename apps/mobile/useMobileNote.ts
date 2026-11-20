import { useCallback, useEffect, useRef, useState } from "react";
import { noteTargetKey, type NoteTarget } from "../../src/noteDrafts";
import { errorText, mobile } from "./api";
import { useResource } from "./useResource";
import { useDurableDraft } from "./useDurableDraft";

const SAVE_DELAY = 600;

export function useMobileNote(target: NoteTarget, revision: number, composing: boolean) {
  const key = noteTargetKey(target);
  const [reload, setReload] = useState(0);
  const resource = useResource(() => mobile.note(target), [key, revision, reload]);
  const journal = useDurableDraft(key);
  const { draft, latest, write } = journal;
  const inFlight = useRef<Promise<boolean> | null>(null);
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string>();
  const refresh = useCallback(() => setReload(value => value + 1), []);
  const note = resource.data?.note ?? null;
  const title = draft?.title ?? note?.title ?? "";
  const body = draft?.body ?? note?.body ?? "";
  const stale = !!draft && resource.data !== undefined && (draft.base !== (note?.updatedAt ?? null) || !!note?.trashed);

  function edit(text: { title?: string; body?: string }) {
    setSaveError(undefined);
    void write({ title, body, base: draft ? draft.base : note?.updatedAt ?? null, ...text });
  }

  const save = useCallback(async (overwrite = false): Promise<boolean> => {
    const previous = inFlight.current;
    if (previous) {
      await previous;
      return latest.current === null;
    }
    const sent = latest.current;
    if (!sent) return true;
    if (!resource.data || resource.data.conflicts.length || note?.trashed || (stale && !overwrite)) return false;
    if (target.kind === "note" && !note) return false;

    setSaving(true);
    setSaveError(undefined);
    const expected = overwrite ? note?.updatedAt ?? null : sent.base;
    const promise = mobile.saveNote(target, sent.title, sent.body, expected).then(result => {
      if (!result.saved) {
        resource.setData({ note: result.note, document: null, links: null, conflicts: [] });
        refresh();
        return false;
      }
      const saved = result.note;
      resource.setData(current => current ? { ...current, note: saved, document: null } : current);
      if (latest.current === sent) void write(null, sent);
      else if (latest.current) {
        const previous = latest.current;
        void write({ ...previous, base: saved?.updatedAt ?? null }, previous);
      }
      refresh();
      return latest.current === null;
    }).catch(failure => {
      setSaveError(errorText(failure));
      refresh();
      return false;
    }).finally(() => {
      inFlight.current = null;
      setSaving(false);
    });
    inFlight.current = promise;
    return promise;
  }, [latest, note, refresh, resource, stale, target, write]);

  useEffect(() => {
    if (!draft || composing || stale || saving || saveError || resource.data?.conflicts.length) return;
    // Body autosave never commits a pending rename.
    if (target.kind === "note" && draft.title !== note?.title) return;
    const timer = window.setTimeout(() => void save(), SAVE_DELAY);
    return () => window.clearTimeout(timer);
  }, [composing, draft, note?.title, resource.data?.conflicts.length, save, saveError, saving, stale, target.kind]);

  return {
    ...resource, note, title, body, draft, stale, saving, saveError,
    draftError: journal.error, draftReady: journal.ready, draftPending: journal.pending, retryDraft: journal.retry,
    edit, save, refresh, discard: () => { if (draft) void write(null, draft); },
  };
}
