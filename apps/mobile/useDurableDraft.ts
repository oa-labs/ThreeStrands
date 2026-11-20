import { useCallback, useEffect, useRef, useState } from "react";
import type { NoteDraft } from "../../src/noteDrafts";
import { errorText, mobile } from "./api";

// A new editor can reopen while the previous one is still committing its
// last input. Wait for those writes before restoring from SQLite.
const writes = new Map<string, Promise<void>>();

export function useDurableDraft(key: string) {
  const [draft, setDraft] = useState<NoteDraft | null>(null);
  const [ready, setReady] = useState(false);
  const [error, setError] = useState<string>();
  const [pending, setPending] = useState(false);
  const [retry, setRetry] = useState(0);
  const latest = useRef<NoteDraft | null>(null);

  useEffect(() => {
    let current = true;
    const load = async () => {
      try {
        await writes.get(key);
        const restored = await mobile.draft(key);
        if (!current) return;
        latest.current = restored;
        setDraft(restored);
        setReady(true);
        setError(undefined);
      } catch (failure) {
        if (current) setError(errorText(failure));
      }
    };
    void load();
    return () => { current = false; };
  }, [key, retry]);

  const write = useCallback((value: NoteDraft | null, expected: NoteDraft | null = null) => {
    latest.current = value;
    setDraft(value);
    setPending(true);
    const previous = writes.get(key);
    const operation = Promise.resolve(previous).catch(() => undefined).then(() =>
      mobile.writeDraft(key, value, expected),
    );
    writes.set(key, operation);
    void operation.then(
      () => { if (writes.get(key) === operation) setError(undefined); },
      failure => { if (writes.get(key) === operation) setError(errorText(failure)); },
    ).finally(() => {
      if (writes.get(key) !== operation) return;
      writes.delete(key);
      setPending(false);
    });
    return operation;
  }, [key]);

  return { draft, ready, error, pending, latest, write, retry: () => setRetry(value => value + 1) };
}
