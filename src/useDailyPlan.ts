import { useCallback, useEffect, useRef, useState } from "react";
import { mailClient } from "./data/client";
import type { DailyPlan } from "./domain";
import { errorMessage, logBackgroundFailure } from "./errors";
import { useNoteChanges } from "./noteMutations";

/**
 * The plan in `date`'s daily note, read again whenever a note changes (here
 * or by sync) and whenever `refreshKey` changes, such as after a task does.
 * A failed read keeps the plan last read and reports the error.
 */
export function useDailyPlan(date: string, refreshKey: string) {
  const [plan, setPlan] = useState<DailyPlan | null>(null);
  const [error, setError] = useState<string | null>(null);
  const version = useRef(0);
  const reload = useCallback(async () => {
    const current = ++version.current;
    try {
      const next = await mailClient.dailyPlan(date);
      if (current !== version.current) return;
      setPlan(next);
      setError(null);
    } catch (reason) {
      if (current !== version.current) return;
      setError(errorMessage(reason));
    }
  }, [date]);
  const cancel = useCallback(() => { version.current++; }, []);
  useEffect(() => {
    void reload().catch(logBackgroundFailure("Daily plan"));
    return cancel;
  }, [reload, refreshKey, cancel]);
  const changed = useCallback(() => { void reload().catch(logBackgroundFailure("Daily plan")); }, [reload]);
  useNoteChanges(changed);
  return { plan: plan?.date === date ? plan : null, error, reload };
}
