import { useCallback, useEffect, useRef, useState } from "react";
import { mailClient } from "./data/client";
import type { DailyPlan, PlanReview } from "./domain";
import { errorMessage } from "./errors";
import { saveDailyPlan } from "./noteCapture";
import { prepareNoteMutation } from "./noteMutations";
import { noteTargetKey } from "./noteDrafts";
import { isActiveTaskStatus } from "./taskViews";
import { decide, discardRecovery, hasChoices, markMissing, planChanges, readRecovery, rebase, startRoutine, writeRecovery, type RoutineItem, type RoutineState } from "./planRoutine";
import { planItemKey, type PlanAddition } from "./dailyPlan";
import type { BlockTarget } from "./todayBooking";

/** An open task the Choose step's search can add. */
export type SearchableTask = { key: string; title: string; detail: string | null; addition: PlanAddition; block: BlockTarget };

/** Whether Review has anything to show; with neither column, the step is skipped. */
export function reviewHasContent(review: PlanReview | null): boolean {
  return Boolean(review && (review.done.length || review.alsoDone.length || review.meetings?.length || review.didntGetTo.length));
}

/**
 * Plan Today's controller for one plan date. It reads the plan and its
 * review once, after saving any open typing in that day's note, and starts
 * from the unfinished routine this device kept for the date, if any
 * (`restored`), re-based on the plan as it is now. Choices are kept locally
 * as they change; nothing is written to the note until `save`.
 */
export function usePlanRoutine(date: string) {
  const [state, setState] = useState<RoutineState | null>(null);
  const [plan, setPlan] = useState<DailyPlan | null>(null);
  const [review, setReview] = useState<PlanReview | null>(null);
  const [reviewError, setReviewError] = useState<string | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [tasks, setTasks] = useState<SearchableTask[]>([]);
  const [restored, setRestored] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  // After a save or a discard, nothing more is kept for recovery.
  const finished = useRef(false);

  useEffect(() => {
    let current = true;
    finished.current = false;
    void (async () => {
      try {
        await prepareNoteMutation(noteTargetKey({ kind: "daily", date }));
        const [planResult, reviewResult, taskResult, noteTaskResult] = await Promise.allSettled([
          mailClient.dailyPlan(date), mailClient.planReview(date), mailClient.listTasks(), mailClient.listNoteTasks(),
        ]);
        if (!current) return;
        if (planResult.status === "rejected") throw planResult.reason;
        const loaded = planResult.value;
        const openTasks = taskResult.status === "fulfilled" ? taskResult.value.filter((task) => isActiveTaskStatus(task.status)) : null;
        const openNoteTasks = noteTaskResult.status === "fulfilled" ? noteTaskResult.value.filter((task) => !task.checked && !task.conflicted) : null;
        const searchable: SearchableTask[] = [
          ...(openTasks ?? []).map((task) => {
            const reference = { version: 1 as const, kind: "task" as const, id: task.id };
            return { key: planItemKey(reference), title: task.title, detail: task.accountId, addition: { kind: "link" as const, reference, label: task.title }, block: { title: task.title, sourceTaskId: task.id, accountId: task.accountId } };
          }),
          ...(openNoteTasks ?? []).map((task) => {
            const reference = { version: 1 as const, kind: "task" as const, id: task.markerId, note: task.noteId };
            return { key: planItemKey(reference), title: task.title, detail: task.noteTitle, addition: { kind: "link" as const, reference, label: task.title }, block: { title: task.title, sourceTaskId: task.id, accountId: null } };
          }),
        ];
        const known = new Set(searchable.map((task) => task.key));
        // A restored item whose task has gone since is marked, not silently saved.
        const listed = openTasks !== null && openNoteTasks !== null;
        const present = (item: RoutineItem) => !listed || item.existing || item.carried !== null || item.addition?.kind !== "link"
          || item.addition.reference.kind !== "task" || known.has(item.key ?? "");
        const recovered = readRecovery(date);
        let next = recovered ? rebase(markMissing(recovered, present), loaded) : startRoutine(date, loaded);
        // A fresh routine starts with what was set aside for this very day, such as items carried to tomorrow in the evening, chosen as Carry.
        if (!recovered && reviewResult.status === "fulfilled") {
          for (const row of reviewResult.value.didntGetTo) {
            if (row.marker?.kind === "deferred" && row.marker.date === date && row.state !== "unavailable") next = decide(next, row, { kind: "carry" });
          }
        }
        setPlan(loaded);
        setTasks(searchable);
        if (reviewResult.status === "fulfilled") setReview(reviewResult.value);
        else setReviewError(errorMessage(reviewResult.reason));
        if (!recovered && !(reviewResult.status === "fulfilled" && reviewHasContent(reviewResult.value))) next.step = "choose";
        setRestored(recovered ? recovered.savedAt : null);
        setState(next);
      } catch (reason) {
        if (current) setLoadError(errorMessage(reason));
      }
    })();
    return () => { current = false; };
  }, [date]);

  // Kept on this device as choices change, so a restart doesn't lose them.
  useEffect(() => {
    if (state && !finished.current && hasChoices(state)) writeRecovery(state);
  }, [state]);

  const update = useCallback((change: (state: RoutineState) => RoutineState) => setState((current) => current ? change(current) : current), []);

  /** Starts again from the plan, forgetting the kept choices. */
  const discard = useCallback(() => {
    discardRecovery(date);
    setRestored(null);
    setState((current) => current ? { ...startRoutine(date, plan), step: current.step } : current);
  }, [date, plan]);

  /** Closing without saving: keep the choices for later, or forget them. */
  const leave = useCallback((keep: boolean) => {
    if (keep && state && hasChoices(state)) writeRecovery(state);
    else { finished.current = true; discardRecovery(date); }
  }, [date, state]);

  /** Writes the plan in one guarded save; the result's `undo` reverses it while the note is unchanged. */
  const save = useCallback(async () => {
    if (!state || saving) return null;
    setSaving(true);
    setSaveError(null);
    try {
      const result = await saveDailyPlan(date, state.base, state.operationId, planChanges(state));
      finished.current = true;
      discardRecovery(date);
      return result;
    } catch (reason) {
      // The choices stay, here and on this device, so the save can be tried again.
      setSaveError(errorMessage(reason));
      return null;
    } finally {
      setSaving(false);
    }
  }, [date, saving, state]);

  return { state, plan, review, reviewError, loadError, tasks, restored, saving, saveError, update, discard, leave, save };
}
