import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useState } from "react";
import { errorMessage } from "./errors";

/** `pending` key for operations that only need a section-wide busy flag. */
export const ANY_OPERATION = "*";

/**
 * Tracks one in-flight settings operation at a time. `pending` holds the key
 * of the item being worked on (an account email, a split inbox id, or
 * `ANY_OPERATION` when the section only needs a busy flag), and a failure's
 * message lands in `error`, with `errorKey` naming the operation that
 * produced it so a section can show the message next to its control.
 * Operations may also set `error` themselves to report a non-exceptional
 * problem such as a cancelled picker, optionally under a key.
 *
 * Sections backed by a status snapshot pass `refresh`; `act`/`actFor` then
 * reload that snapshot after each successful operation.
 */
export function useSettingsOperation(refresh?: () => Promise<unknown>) {
  const [pending, setPending] = useState<string | null>(null);
  const [failure, setFailure] = useState<{ message: string; key: string } | null>(null);
  const setError = useCallback((message: string | null, key: string = ANY_OPERATION) => {
    setFailure(message === null ? null : { message, key });
  }, []);
  const runFor = useCallback((key: string, operation: () => Promise<unknown>) => {
    setPending(key);
    setFailure(null);
    void operation()
      .catch((reason: unknown) => setFailure({ message: errorMessage(reason), key }))
      .finally(() => setPending(null));
  }, []);
  const run = useCallback((operation: () => Promise<unknown>) => runFor(ANY_OPERATION, operation), [runFor]);
  const actFor = useCallback((key: string, operation: () => Promise<unknown>) => runFor(key, async () => {
    await operation();
    await refresh?.();
  }), [refresh, runFor]);
  const act = useCallback((operation: () => Promise<unknown>) => actFor(ANY_OPERATION, operation), [actFor]);
  return {
    pending,
    busy: pending !== null,
    error: failure?.message ?? null,
    errorKey: failure?.key ?? null,
    setError,
    run,
    runFor,
    act,
    actFor,
  };
}

/**
 * Loads a status snapshot on mount and reloads it whenever the native side
 * emits `eventName`. The subscription is released even if the component
 * unmounts before `listen` resolves.
 */
export function useLiveStatus(eventName: string, refresh: () => Promise<unknown>, onError: (reason: unknown) => void) {
  useEffect(() => {
    void refresh().catch(onError);
    if (!("__TAURI_INTERNALS__" in window)) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listen(eventName, () => void refresh().catch(onError)).then((stop) => {
      if (disposed) stop();
      else unlisten = stop;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [eventName, onError, refresh]);
}

/** Returns `items` with the entry at `index` swapped one step, or null at an edge. */
export function moveItem<T>(items: readonly T[], index: number, direction: -1 | 1): T[] | null {
  const target = index + direction;
  if (index < 0 || index >= items.length || target < 0 || target >= items.length) return null;
  const next = [...items];
  [next[index], next[target]] = [next[target]!, next[index]!];
  return next;
}
