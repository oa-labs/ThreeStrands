import { useMemo, useRef, useState } from "react";
import { Modal } from "./AppChrome";
import type { CreateGoalRequest, Goal, GoalHorizon, GoalStatus, UpdateGoalRequest } from "./domain";
import { errorMessage } from "./errors";
import { formatPeriod, GOAL_HORIZON_LABELS, GOAL_HORIZONS, parentCandidates, periodFor, shiftPeriod } from "./goals";

type GoalDrafts = {
  accountId: string;
  title: string;
  notes: string;
  horizon: GoalHorizon;
  period: string;
  parentGoalId: string;
  status: GoalStatus;
};

/** The first day of a period, so a goal moved to another horizon keeps its place in time. */
function periodStart(period: string): Date {
  const [year, part] = period.split("-");
  const month = !part ? 0 : part[0] === "H" ? (Number(part[1]) - 1) * 6 : (Number(part[1]) - 1) * 3;
  return new Date(Number(year), month, 1);
}

/** The current period and its neighbors, plus `selected` when it lies outside them. */
function periodOptions(horizon: GoalHorizon, selected: string, now: Date): string[] {
  let period = periodFor(horizon, now);
  for (let step = 0; step < 2; step += 1) period = shiftPeriod(horizon, period, -1);
  const options = [period];
  for (let step = 0; step < 6; step += 1) options.push(period = shiftPeriod(horizon, period, 1));
  return options.includes(selected) ? options : [...options, selected].sort();
}

const STATUS_LABELS: Record<GoalStatus, string> = { active: "Active", achieved: "Achieved", dropped: "Dropped" };

/**
 * Creates a goal, or edits one with every field saved as the user leaves it,
 * like the task dialog: Escape and Cmd+Enter save what is pending and close.
 */
export function GoalDialog({
  goal,
  goals,
  accountOptions,
  defaultAccountId,
  now = new Date(),
  onCreate,
  onUpdate,
  onDelete,
  onClose,
}: {
  /** The goal being edited; omitted to create one. */
  goal?: Goal | null;
  goals: readonly Goal[];
  /** Accounts to choose from when creating with no account selected. */
  accountOptions: readonly string[];
  defaultAccountId: string | null;
  now?: Date;
  onCreate(request: CreateGoalRequest): Promise<void>;
  onUpdate(request: Omit<UpdateGoalRequest, "id">): Promise<void>;
  onDelete(): Promise<void>;
  onClose(): void;
}) {
  const editing = Boolean(goal);
  const [drafts, setDrafts] = useState<GoalDrafts>(() => goal ? {
    accountId: goal.accountId,
    title: goal.title,
    notes: goal.notes ?? "",
    horizon: goal.horizon,
    period: goal.period,
    parentGoalId: goal.parentGoalId ?? "",
    status: goal.status,
  } : {
    accountId: defaultAccountId ?? (accountOptions.length === 1 ? accountOptions[0] : ""),
    title: "",
    notes: "",
    horizon: "quarter",
    period: periodFor("quarter", now),
    parentGoalId: "",
    status: "active",
  });
  const draftsRef = useRef(drafts);
  draftsRef.current = drafts;
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [confirmingDelete, setConfirmingDelete] = useState(false);
  const titleRef = useRef<HTMLInputElement>(null);

  const candidates = useMemo(
    () => parentCandidates(goals, { id: goal?.id ?? "", accountId: drafts.accountId, horizon: drafts.horizon, period: drafts.period })
      .filter((candidate) => candidate.status === "active" || candidate.id === drafts.parentGoalId),
    [drafts.accountId, drafts.horizon, drafts.parentGoalId, drafts.period, goal?.id, goals],
  );

  /** Saves the fields of an existing goal that differ from it. */
  const commit = (next: GoalDrafts = draftsRef.current) => {
    if (!goal) return;
    const request: Omit<UpdateGoalRequest, "id"> = {};
    const title = next.title.trim();
    if (!title) {
      setError("A goal needs a title.");
    } else if (title !== goal.title) {
      request.title = title;
    }
    const notes = next.notes.trim() || null;
    if (notes !== goal.notes) request.notes = notes;
    if (next.horizon !== goal.horizon) request.horizon = next.horizon;
    if (next.period !== goal.period) request.period = next.period;
    if (next.status !== goal.status) request.status = next.status;
    if ((next.parentGoalId || null) !== goal.parentGoalId) request.parentGoalId = next.parentGoalId || null;
    if (Object.keys(request).length === 0) return;
    if (title) setError(null);
    onUpdate(request).catch((reason) => setError(errorMessage(reason)));
  };

  const change = (patch: Partial<GoalDrafts>, save = false) => {
    let next = { ...draftsRef.current, ...patch };
    // A new horizon or period can leave the supported goal outside it; unlink it in the same change.
    if ((patch.horizon || patch.period) && next.parentGoalId
      && !parentCandidates(goals, { id: goal?.id ?? "", accountId: next.accountId, horizon: next.horizon, period: next.period }).some((candidate) => candidate.id === next.parentGoalId)) {
      next = { ...next, parentGoalId: "" };
    }
    draftsRef.current = next;
    setDrafts(next);
    if (save) commit(next);
  };

  const close = () => {
    commit();
    onClose();
  };

  const create = async () => {
    const current = draftsRef.current;
    if (!current.title.trim() || !current.accountId || busy) return;
    setBusy(true);
    setError(null);
    try {
      await onCreate({
        accountId: current.accountId,
        title: current.title.trim(),
        notes: current.notes.trim() || null,
        horizon: current.horizon,
        period: current.period,
        parentGoalId: current.parentGoalId || null,
      });
    } catch (reason) {
      setError(errorMessage(reason));
      setBusy(false);
    }
  };

  const remove = async () => {
    setBusy(true);
    try {
      await onDelete();
    } catch (reason) {
      setError(errorMessage(reason));
      setBusy(false);
    }
  };

  return (
    <Modal title={editing ? "Goal" : "Add goal"} className="task-editor-modal task-detail-modal goal-modal" onClose={editing ? close : onClose} initialFocusRef={titleRef}>
      <form className="modal-form task-detail-form" onSubmit={(event) => { event.preventDefault(); if (!editing) void create(); }} onKeyDown={(event) => {
        if ((event.metaKey || event.ctrlKey) && event.key === "Enter") {
          event.preventDefault();
          if (editing) close();
          else void create();
        } else if (editing && event.key === "Enter" && event.target instanceof HTMLInputElement) {
          event.preventDefault();
          commit();
        }
      }}>
        {!editing && !defaultAccountId && accountOptions.length > 1 ? <label>
          <span>Account</span>
          <select value={drafts.accountId} onChange={(event) => change({ accountId: event.target.value, parentGoalId: "" })} required>
            <option value="">Choose an account</option>
            {accountOptions.map((email) => <option key={email} value={email}>{email}</option>)}
          </select>
        </label> : null}
        <label className="task-detail-title-field">
          <span>Goal</span>
          <input ref={titleRef} value={drafts.title} onChange={(event) => change({ title: event.target.value })} onBlur={() => commit()} maxLength={240} placeholder="What do you want to achieve?" />
        </label>
        <div className="task-detail-row">
          <label><span>Horizon</span><select value={drafts.horizon} onChange={(event) => {
            const horizon = event.target.value as GoalHorizon;
            change({ horizon, period: periodFor(horizon, periodStart(draftsRef.current.period)) }, true);
          }}>
            {[...GOAL_HORIZONS].reverse().map((horizon) => <option key={horizon} value={horizon}>{GOAL_HORIZON_LABELS[horizon]}</option>)}
          </select></label>
          <label><span>Period</span><select value={drafts.period} onChange={(event) => change({ period: event.target.value }, true)}>
            {periodOptions(drafts.horizon, drafts.period, now).map((period) => <option key={period} value={period}>{formatPeriod(period)}</option>)}
          </select></label>
        </div>
        {drafts.horizon !== "year" ? <label>
          <span>Supports</span>
          <select value={drafts.parentGoalId} onChange={(event) => change({ parentGoalId: event.target.value }, true)}>
            <option value="">No longer-term goal</option>
            {candidates.map((candidate) => <option key={candidate.id} value={candidate.id}>{candidate.title} ({formatPeriod(candidate.period)})</option>)}
          </select>
        </label> : null}
        {editing ? <label><span>Status</span><select value={drafts.status} onChange={(event) => change({ status: event.target.value as GoalStatus }, true)}>
          {(Object.keys(STATUS_LABELS) as GoalStatus[]).map((status) => <option key={status} value={status}>{STATUS_LABELS[status]}</option>)}
        </select></label> : null}
        <label>
          <span>Notes</span>
          <textarea value={drafts.notes} onChange={(event) => change({ notes: event.target.value })} onBlur={() => commit()} rows={4} maxLength={8000} placeholder="Why it matters, how you'll know it's done" />
        </label>
        {error ? <p className="form-error" role="alert">{error}</p> : null}
        {editing ? <footer className="task-detail-hints goal-dialog-footer">
          <span>Changes save automatically</span>
          {confirmingDelete ? <span className="goal-delete-confirm">
            Delete this goal? Its tasks stay and are unlinked.
            <button type="button" className="btn btn-sm btn-danger" disabled={busy} onClick={() => void remove()}>Delete</button>
            <button type="button" className="btn btn-sm" onClick={() => setConfirmingDelete(false)}>Keep</button>
          </span> : <button type="button" className="btn btn-sm btn-danger" onClick={() => setConfirmingDelete(true)}>Delete goal</button>}
        </footer> : <div className="modal-form-actions">
          <button type="button" className="btn" onClick={onClose}>Cancel</button>
          <button type="submit" className="btn btn-primary" disabled={busy || !drafts.title.trim() || !drafts.accountId}>{busy ? "Adding…" : "Add goal"}</button>
        </div>}
      </form>
    </Modal>
  );
}
