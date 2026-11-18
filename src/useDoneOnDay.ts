import { useCallback, useEffect, useRef, useState } from "react";
import { mailClient } from "./data/client";
import type { CreatedNote, SentInRange } from "./domain";
import { addDays, dateInputValue, startOfLocalDay } from "./calendarTime";
import { errorMessage } from "./errors";
import { useNoteChanges } from "./noteMutations";

/** Most notes the Done card reads for one day. */
export const DONE_NOTES_LIMIT = 100;

type Loaded<T> = { key: string; items: T[]; error: string | null };
const empty = <T,>(key: string): Loaded<T> => ({ key, items: [], error: null });

/** Reads one source for a day, ignoring answers that arrive after the day, account or refresh moved on. */
function useDaySource<T>(key: string, enabled: boolean, refreshKey: string, read: () => Promise<T[]>, watchNotes = false) {
  const [state, setState] = useState<Loaded<T>>(() => empty(key));
  const [loading, setLoading] = useState(enabled);
  const version = useRef(0);
  const readRef = useRef(read);
  readRef.current = read;
  const reload = useCallback(async () => {
    const request = ++version.current;
    if (!enabled) { setLoading(false); return; }
    setLoading(true);
    try {
      const items = await readRef.current();
      if (request === version.current) setState({ key, items, error: null });
    } catch (reason) {
      if (request === version.current) setState((previous) => ({ key, items: previous.key === key ? previous.items : [], error: errorMessage(reason) }));
    } finally {
      if (request === version.current) setLoading(false);
    }
  }, [key, enabled]);
  const cancel = useCallback(() => { version.current++; }, []);
  useEffect(() => {
    void reload();
    return cancel;
  }, [reload, refreshKey, cancel]);
  useNoteChanges(watchNotes ? reload : noop);
  const current = state.key === key ? state : empty<T>(key);
  return { ...current, loading: enabled && (loading || state.key !== key) };
}
const noop = () => {};

/**
 * What the Done card reads beyond tasks for `day`: conversations the user sent in
 * and notes created on that local day. Each source fails on its own.
 */
export function useDoneOnDay(day: Date, accountId: string | null, refreshKey: string, enabled: boolean) {
  const start = startOfLocalDay(day);
  const startIso = start.toISOString();
  const endIso = addDays(start, 1).toISOString();
  const key = `${dateInputValue(start)}|${accountId ?? ""}`;
  const sent = useDaySource<SentInRange>(key, enabled, refreshKey, () => mailClient.listSentBetween(accountId ?? undefined, startIso, endIso));
  const notes = useDaySource<CreatedNote>(key, enabled, refreshKey, () => mailClient.listNotesCreatedBetween(startIso, endIso, DONE_NOTES_LIMIT), true);
  return { sent: sent.items, notes: notes.items, errors: { emails: sent.error ?? undefined, notes: notes.error ?? undefined }, loading: sent.loading || notes.loading };
}
