import { useState } from "react";
import { Modal } from "./AppChrome";
import type { Goal, ThreadTask, UpdateGoalRequest } from "./domain";
import { errorMessage } from "./errors";
import { carryForwardRequest, formatPeriod, GOAL_HORIZON_LABELS, goalProgress, goalsToReview, periodFor } from "./goals";

/**
 * Settles active goals whose period has ended: each one is marked achieved,
 * dropped, or carried into the current period of its horizon. A settled goal
 * leaves the list; closing early leaves the rest for the next review.
 */
export function GoalReviewDialog({
  goals,
  tasks,
  now = new Date(),
  onUpdate,
  onClose,
}: {
  goals: readonly Goal[];
  tasks: readonly ThreadTask[];
  now?: Date;
  onUpdate(goalId: string, request: Omit<UpdateGoalRequest, "id">): Promise<void>;
  onClose(): void;
}) {
  const [busyId, setBusyId] = useState<string | null>(null);
  const [errors, setErrors] = useState<Record<string, string>>({});
  const pending = goalsToReview(goals, now);

  const settle = async (goal: Goal, request: Omit<UpdateGoalRequest, "id">) => {
    setBusyId(goal.id);
    setErrors((current) => {
      const next = { ...current };
      delete next[goal.id];
      return next;
    });
    try {
      await onUpdate(goal.id, request);
    } catch (reason) {
      setErrors((current) => ({ ...current, [goal.id]: errorMessage(reason) }));
    } finally {
      setBusyId(null);
    }
  };

  return (
    <Modal title="Review goals" className="task-editor-modal task-detail-modal goal-review-modal" onClose={onClose}>
      <div className="modal-form goal-review">
        {pending.length ? <>
          <p className="goal-review-intro">These goals' periods have ended. Settle each one; its tasks keep their link either way.</p>
          <ul className="goal-review-list">
            {pending.map((goal) => {
              const progress = goalProgress(goal, tasks);
              const nextPeriod = formatPeriod(periodFor(goal.horizon, now));
              const busy = busyId === goal.id;
              return <li key={goal.id} aria-label={goal.title}>
                <div className="goal-review-goal">
                  <strong>{goal.title}</strong>
                  <span>{GOAL_HORIZON_LABELS[goal.horizon]} · {formatPeriod(goal.period)} · {progress.open} open · {progress.done} done</span>
                </div>
                <div className="goal-review-choices">
                  <button type="button" disabled={busy} onClick={() => void settle(goal, { status: "achieved" })}>Achieved</button>
                  <button type="button" disabled={busy} onClick={() => void settle(goal, { status: "dropped" })}>Dropped</button>
                  <button type="button" disabled={busy} onClick={() => void settle(goal, carryForwardRequest(goal, goals, now))}>Carry to {nextPeriod}</button>
                </div>
                {errors[goal.id] ? <p className="form-error" role="alert">{errors[goal.id]}</p> : null}
              </li>;
            })}
          </ul>
        </> : <p className="goal-review-intro">All caught up. Every goal from a past period is settled.</p>}
        <div className="modal-form-actions"><button type="button" className="btn btn-primary" onClick={onClose}>{pending.length ? "Finish later" : "Done"}</button></div>
      </div>
    </Modal>
  );
}
