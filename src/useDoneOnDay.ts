import { useCallback, useEffect, useRef, useState } from "react";
import { mailClient } from "./data/client";
import type { CreatedScheduleEvent, DoneNote, SentInRange } from "./domain";
import { addDays, dateInputValue, startOfLocalDay } from "./calendarTime";
import { errorMessage } from "./errors";
import { useNoteChanges } from "./noteMutations";

/** Most notes the Done card reads for one day. */
export const DONE_NOTES_LIMIT = 100;

type Loaded<T> = { key: string; items: T[]; error: string | null; settled: boolean };
const empty = <T,>(key: string): Loaded<T> => ({ key, items: [], error: null, settled: false });

/**
 * Reads one source for a day, ignoring answers that arrive after the day, account or refresh moved on.
 * It reports loading only until the first answer for a day and account; background refreshes keep the
 * last answer on screen so the card never flickers back to a loading message.
 */
function useDaySource<T>(key: string, enabled: boolean, refreshKey: string, read: () => Promise<T[]>, watchNotes = false) {
  const [state, setState] = useState<Loaded<T>>(() => empty(key));
  const version = useRef(0);
  const readRef = useRef(read);
  readRef.current = read;
  const reload = useCallback(async () => {
    const request = ++version.current;
    if (!enabled) return;
    try {
      const items = await readRef.current();
      if (request === version.current) setState({ key, items, error: null, settled: true });
    } catch (reason) {
      if (request === version.current) setState((previous) => ({ key, items: previous.key === key ? previous.items : [], error: errorMessage(reason), settled: true }));
    }
  }, [key, enabled]);
  const cancel = useCallback(() => { version.current++; }, []);
  useEffect(() => {
    void reload();
    return cancel;
  }, [reload, refreshKey, cancel]);
  useNoteChanges(watchNotes ? reload : noop);
  const current = state.key === key ? state : empty<T>(key);
  return { ...current, loading: enabled && !current.settled };
}
const noop = () => {};

/**
 * What the Done card reads beyond tasks for `day`: conversations the user sent in
 * meetings set up and notes created or starred notes last edited on that local day.
 * Each source fails on its own.
 */
export function useDoneOnDay(day: Date, accountId: string | null, refreshKey: string, enabled: boolean) {
  const start = startOfLocalDay(day);
  const startIso = start.toISOString();
  const endIso = addDays(start, 1).toISOString();
  const key = `${dateInputValue(start)}|${accountId ?? ""}`;
  const sent = useDaySource<SentInRange>(key, enabled, refreshKey, () => mailClient.listSentBetween(accountId ?? undefined, startIso, endIso));
  const notes = useDaySource<DoneNote>(key, enabled, refreshKey, () => mailClient.listNotesDoneBetween(startIso, endIso, DONE_NOTES_LIMIT), true);
  const meetings = useDaySource<CreatedScheduleEvent>(key, enabled, refreshKey, async () => {
    const result = await mailClient.listEventsCreatedBetween(startIso, endIso, Intl.DateTimeFormat().resolvedOptions().timeZone);
    // A calendar that could not be read is reported, but the meetings from the others still show.
    if (result.errors.length && !result.events.length) throw new Error(result.errors.join("; "));
    return result.events;
  });
  return {
    sent: sent.items, notes: notes.items, meetings: meetings.items,
    errors: { emails: sent.error ?? undefined, notes: notes.error ?? undefined, meetings: meetings.error ?? undefined },
    loading: sent.loading || notes.loading || meetings.loading,
  };
}
