import { useEffect } from "react";
import { listen } from "@tauri-apps/api/event";
import { logBackgroundFailure } from "./errors";
import { mailClient } from "./data/client";
import { readNoteDraft } from "./noteDrafts";

const owners = new Map<string, { flush(): Promise<void>; unsaved(): boolean; refresh(): Promise<void>; version?(): string | null }>();
export const NOTES_CHANGED = "threestrands-notes-changed";
export function registerNoteOwner(key: string, owner: { flush(): Promise<void>; unsaved(): boolean; refresh(): Promise<void>; version?(): string | null }) {
  owners.set(key, owner);
  return () => { if (owners.get(key) === owner) owners.delete(key); };
}
export async function prepareNoteMutation(id: string) {
  const owner = owners.get(id);
  const before = owner?.version?.();
  if (owner) { await owner.flush(); if (owner.unsaved()) throw new Error("This note has unsaved changes. Open Note to resolve them first."); }
  if (readNoteDraft(id)) throw new Error("This note has a saved draft. Open Note to resolve it first.");
  return { before, after: owner?.version?.() };
}
export async function notesChanged(id: string) {
  await owners.get(id)?.refresh();
  window.dispatchEvent(new CustomEvent(NOTES_CHANGED, { detail: id }));
}
export async function changeNoteItem(noteId: string, markerId: string, expected: string, patch: { checked?: boolean; task?: boolean }) {
  const prepared = await prepareNoteMutation(noteId);
  // Advance only through this editor's own successful flush. A card based
  // on a different version stays stale and the native guard refuses it.
  const version = prepared.before === expected ? prepared.after ?? expected : expected;
  const note = await mailClient.patchNoteItem(noteId, markerId, version, patch.checked, patch.task);
  await notesChanged(noteId);
  return note;
}

/** Local mutations and a peer's projected note changes share one refresh path. */
export function useNoteChanges(changed: () => void) {
  useEffect(() => {
    window.addEventListener(NOTES_CHANGED, changed);
    let active = true;
    let stop: (() => void) | undefined;
    if ("__TAURI_INTERNALS__" in window) {
      void listen("replicated-sync-status", changed).then((unlisten) => {
        if (active) stop = unlisten;
        else unlisten();
      }).catch(logBackgroundFailure("Note changes listener"));
    }
    return () => { active = false; stop?.(); window.removeEventListener(NOTES_CHANGED, changed); };
  }, [changed]);
}
