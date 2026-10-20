import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import type { Draft, ServerDraftEvent, ServerDraftStatus } from "./correspondence";
import { mailClient } from "./data/client";
import { normalizeAddressList } from "./emailAddress";
import { logBackgroundFailure } from "./errors";

// Save after a pause, with a periodic safety net for continuous typing.
const AUTOSAVE_DEBOUNCE_MS = 300;
const AUTOSAVE_INTERVAL_MS = 3_000;

export type DraftSaveStatus = { text: string; detail?: string; conflict?: boolean };

/** What the composer says once a revision is saved locally. */
export function savedStatus(server?: ServerDraftStatus | null): DraftSaveStatus {
  switch (server?.state) {
    case "pending": return { text: "Saving…" };
    case "retrying": return { text: "Saved on this device · Will sync when online", detail: server.error };
    case "failed": return { text: "Saved on this device · Couldn't save to server", detail: server.error };
    case "conflict": return { text: "Saved on this device · Also changed in another app", conflict: true };
    default: return { text: "Draft saved" };
  }
}

/** Owns draft persistence; the editor supplies body changes only at a save boundary. */
export function useDraftAutosave(
  initial: Draft,
  captureChanges: () => Pick<Draft, "body" | "bodyHtml" | "signature"> | null,
  onError: (message: string) => void,
) {
  const [draft, setDraft] = useState(initial);
  const latest = useRef(initial);
  const generation = useRef(0);
  const savedGeneration = useRef(0);
  const pending = useRef<Promise<Draft> | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const mounted = useRef(true);
  const [status, setStatusValue] = useState<DraftSaveStatus>(() => savedStatus(initial.serverDraft));
  const setStatus = useCallback((next: string | DraftSaveStatus) => setStatusValue(typeof next === "string" ? { text: next } : next), []);

  const captureBody = useCallback(() => {
    const changes = captureChanges();
    if (changes) latest.current = { ...latest.current, ...changes };
  }, [captureChanges]);

  const flush = useCallback(function flush(): Promise<Draft> {
    if (timer.current) clearTimeout(timer.current);
    captureBody();
    // Every caller waits until all edits, including those made during an
    // outstanding write, have been saved. Only one write runs at a time.
    if (pending.current) return pending.current.then(() => generation.current === savedGeneration.current ? latest.current : flush());
    if (generation.current === savedGeneration.current) return Promise.resolve(latest.current);
    const version = generation.current;
    // Preserve header normalization, including repair of older recipient text.
    const snapshot = { ...latest.current, to: normalizeAddressList(latest.current.to), cc: normalizeAddressList(latest.current.cc), bcc: normalizeAddressList(latest.current.bcc) };
    setStatus("Saving…");
    onError("");
    const saving = mailClient.saveDraft(snapshot).then((saved) => {
      savedGeneration.current = version;
      // A save response supplies metadata; it must never replace newer edits.
      latest.current = { ...latest.current, revision: saved.revision, updatedAt: saved.updatedAt };
      if (mounted.current) {
        setDraft(latest.current);
        setStatus(generation.current === version ? savedStatus(saved.serverDraft) : "Unsaved changes");
      }
      return latest.current;
    }).catch((reason) => {
      if (mounted.current) {
        onError(String(reason));
        setStatus("Not saved — retry before closing");
      }
      throw reason;
    }).finally(() => { pending.current = null; });
    pending.current = saving;
    return saving.then(() => generation.current === savedGeneration.current ? latest.current : flush());
  }, [captureBody, onError, setStatus]);

  const markChanged = useCallback(() => {
    generation.current++;
    setStatus("Unsaved changes");
    if (timer.current) clearTimeout(timer.current);
    timer.current = setTimeout(() => { void flush().catch(logBackgroundFailure("Draft autosave")); }, AUTOSAVE_DEBOUNCE_MS);
  }, [flush, setStatus]);

  // The server copy settles after the save that started it; follow it while
  // there are no newer local edits.
  const id = initial.id;
  useEffect(() => {
    if (!("__TAURI_INTERNALS__" in window)) return;
    let listening = true;
    let stop: (() => void) | undefined;
    void listen<ServerDraftEvent>("draft-server-status", ({ payload }) => {
      if (!mounted.current || payload.id !== id || pending.current || generation.current !== savedGeneration.current) return;
      // A confirmation of an older revision says nothing about this one.
      if (payload.status.state === "saved" && payload.revision < latest.current.revision) return;
      setStatus(savedStatus(payload.status));
    })
      .then((unlisten) => { if (listening) stop = unlisten; else unlisten(); })
      .catch(logBackgroundFailure("Draft server status listener"));
    return () => { listening = false; stop?.(); };
  }, [id, setStatus]);

  const edit = useCallback((field: "to" | "cc" | "bcc" | "subject", value: string) => {
    latest.current = { ...latest.current, [field]: value };
    setDraft(latest.current);
    markChanged();
  }, [markChanged]);

  // Attachment and account operations already persist their returned draft.
  const replaceDraft = useCallback((next: Draft) => {
    latest.current = next;
    setDraft(next);
  }, []);
  /** Takes a draft the native side settled, with its server status. */
  const adopt = useCallback((next: Draft) => {
    replaceDraft(next);
    setStatus(savedStatus(next.serverDraft));
  }, [replaceDraft, setStatus]);

  /** Resolves once no save is running; a failed save still counts as finished. */
  const settled = useCallback(async () => {
    while (pending.current) await pending.current.catch(() => undefined);
  }, []);
  const hasUnsavedChanges = useCallback(() => generation.current !== savedGeneration.current, []);
  /** Saves these edits over the newer revision another device wrote. */
  const overwrite = useCallback((revision: number) => {
    latest.current = { ...latest.current, revision };
    markChanged();
  }, [markChanged]);

  useEffect(() => {
    mounted.current = true;
    const beforeUnload = (event: BeforeUnloadEvent) => {
      if (generation.current !== savedGeneration.current) {
        event.preventDefault();
        event.returnValue = "";
      }
    };
    window.addEventListener("beforeunload", beforeUnload);
    const autosave = window.setInterval(() => { void flush().catch(logBackgroundFailure("Draft autosave")); }, AUTOSAVE_INTERVAL_MS);
    return () => {
      mounted.current = false;
      if (timer.current) clearTimeout(timer.current);
      window.clearInterval(autosave);
      window.removeEventListener("beforeunload", beforeUnload);
    };
  }, [flush]);

  return { draft, latest, status, edit, markChanged, captureBody, flush, replaceDraft, adopt, settled, hasUnsavedChanges, overwrite };
}
