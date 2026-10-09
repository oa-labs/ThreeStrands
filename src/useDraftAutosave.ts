import { useCallback, useEffect, useRef, useState } from "react";
import type { Draft } from "./correspondence";
import { mailClient } from "./data/client";
import { normalizeAddressList } from "./emailAddress";
import { logBackgroundFailure } from "./errors";

// Save after a pause, with a periodic safety net for continuous typing.
const AUTOSAVE_DEBOUNCE_MS = 300;
const AUTOSAVE_INTERVAL_MS = 3_000;

/** Owns draft persistence; the editor supplies body changes only at a save boundary. */
export function useDraftAutosave(
  initial: Draft,
  captureChanges: () => Pick<Draft, "body" | "bodyHtml"> | null,
  onError: (message: string) => void,
) {
  const [draft, setDraft] = useState(initial);
  const latest = useRef(initial);
  const generation = useRef(0);
  const savedGeneration = useRef(0);
  const pending = useRef<Promise<Draft> | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const mounted = useRef(true);
  const [status, setStatus] = useState("Saved on this device");

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
        setStatus(generation.current === version ? "Saved on this device" : "Unsaved changes");
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
  }, [captureBody, onError]);

  const markChanged = useCallback(() => {
    generation.current++;
    setStatus("Unsaved changes");
    if (timer.current) clearTimeout(timer.current);
    timer.current = setTimeout(() => { void flush().catch(logBackgroundFailure("Draft autosave")); }, AUTOSAVE_DEBOUNCE_MS);
  }, [flush]);

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

  return { draft, latest, status, edit, markChanged, captureBody, flush, replaceDraft };
}
