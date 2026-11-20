import { useEffect, useMemo, useRef, useState } from "react";
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
  onUpdate(request: Omit<UpdateGoalRequest, "id">): Promise<Goal>;
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
  const savedGoal = useRef(goal);
  const pendingSave = useRef<{ drafts: GoalDrafts; done: Promise<boolean> } | null>(null);
  const closing = useRef(false);
  useEffect(() => { savedGoal.current = goal; }, [goal]);

  const candidates = useMemo(
    () => parentCandidates(goals, { id: goal?.id ?? "", accountId: drafts.accountId, horizon: drafts.horizon, period: drafts.period })
      .filter((candidate) => candidate.status === "active" || candidate.id === drafts.parentGoalId),
    [drafts.accountId, drafts.horizon, drafts.parentGoalId, drafts.period, goal?.id, goals],
  );

  const commit = (next: GoalDrafts = draftsRef.current): Promise<boolean> => {
    const previous = pendingSave.current;
    if (previous?.drafts === next) return previous.done;

    // Diff against the last completed write. A newer draft may undo a field
    // change that an earlier blur is still saving.
    const save = async () => {
      const current = savedGoal.current;
      if (!current) return true;
      const request: Omit<UpdateGoalRequest, "id"> = {};
      const title = next.title.trim();
      if (title && title !== current.title) request.title = title;
      const notes = next.notes.trim() || null;
      if (notes !== current.notes) request.notes = notes;
      if (next.horizon !== current.horizon) request.horizon = next.horizon;
      if (next.period !== current.period) request.period = next.period;
      if (next.status !== current.status) request.status = next.status;
      if ((next.parentGoalId || null) !== current.parentGoalId) request.parentGoalId = next.parentGoalId || null;
      if (!title) setError("A goal needs a title.");
      if (Object.keys(request).length === 0) return true;
      if (title) setError(null);
      try {
        savedGoal.current = await onUpdate(request);
        return true;
      } catch (reason) {
        setError(errorMessage(reason));
        return false;
      }
    };
    const done = previous ? previous.done.then(save) : save();
    pendingSave.current = { drafts: next, done };
    void done.then(() => { if (pendingSave.current?.done === done) pendingSave.current = null; });
    return done;
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
    if (save) void commit(next);
  };

  const close = () => {
    if (closing.current || busy) return;
    closing.current = true;
    const next = draftsRef.current;
    void commit(next).then((saved) => {
      closing.current = false;
      if (saved && next === draftsRef.current) onClose();
    });
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
          void commit();
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
          <input ref={titleRef} value={drafts.title} onChange={(event) => change({ title: event.target.value })} onBlur={() => { void commit(); }} maxLength={240} placeholder="What do you want to achieve?" />
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
            <option value="">No Longer-Term Goal</option>
            {candidates.map((candidate) => <option key={candidate.id} value={candidate.id}>{candidate.title} ({formatPeriod(candidate.period)})</option>)}
          </select>
        </label> : null}
        {editing ? <label><span>Status</span><select value={drafts.status} onChange={(event) => change({ status: event.target.value as GoalStatus }, true)}>
          {(Object.keys(STATUS_LABELS) as GoalStatus[]).map((status) => <option key={status} value={status}>{STATUS_LABELS[status]}</option>)}
        </select></label> : null}
        <label>
          <span>Notes</span>
          <textarea value={drafts.notes} onChange={(event) => change({ notes: event.target.value })} onBlur={() => { void commit(); }} rows={4} maxLength={8000} placeholder="Why it matters, how you'll know it's done" />
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
