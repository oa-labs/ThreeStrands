import { useCallback, useEffect, useRef, useState } from "react";
import { mailClient } from "./data/client";
import { errorMessage } from "./errors";
import { todayTarget } from "./noteDrafts";
import { useNoteChanges } from "./noteMutations";
import type { NoteTaskRecord } from "./noteEntities";

/** Completion history comes from saved tasks, so it survives refresh and reopening Today. */
export function useTodayCompletedNotes(date: string, refreshKey: string) {
  const [state, setState] = useState<{ date: string; tasks: NoteTaskRecord[]; error: string | null }>({ date, tasks: [], error: null });
  const version = useRef(0);
  const reload = useCallback(async () => {
    const request = ++version.current;
    try {
      const tasks = (await mailClient.listNoteTasks()).filter((task) => task.checked && !task.conflicted && task.completedAt && todayTarget(new Date(task.completedAt)).date === date);
      if (request === version.current) setState({ date, tasks, error: null });
    } catch (reason) {
      if (request === version.current) setState((previous) => ({ date, tasks: previous.date === date ? previous.tasks : [], error: errorMessage(reason) }));
    }
  }, [date]);
  const cancel = useCallback(() => { version.current++; }, []);
  useEffect(() => {
    void reload();
    return cancel;
  }, [reload, refreshKey, cancel]);
  useNoteChanges(reload);
  return state.date === date ? state : { date, tasks: [], error: null };
}
