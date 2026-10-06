import { useMemo, useRef, useState } from "react";
import { Modal } from "./AppChrome";
import { convertDueInputValue, isValidTimeZone, listSupportedTimeZones } from "./calendarTime";
import type { Goal, TaskDueKind, TaskKind } from "./domain";
import { errorMessage } from "./errors";
import { formatPeriod, GOAL_HORIZON_LABELS, GOAL_HORIZONS, goalOptionsForTask } from "./goals";
import { MAX_REPEAT_INTERVAL_DAYS } from "./TaskDetailDialog";

export type TaskEditorValues = {
  title: string;
  notes: string | null;
  kind: TaskKind;
  dueKind: TaskDueKind;
  dueValue: string | null;
  timeZone: string | null;
  repeatIntervalDays: number | null;
  /** Present only when the dialog offers goals. */
  goalId?: string | null;
};

type TaskEditorInitial = Partial<TaskEditorValues> & Pick<TaskEditorValues, "title">;

function dateTimeInputValue(value: string | null | undefined): string {
  if (!value) return "";
  const parsed = new Date(value);
  if (Number.isNaN(parsed.getTime())) return value.slice(0, 16);
  const local = new Date(parsed.getTime() - parsed.getTimezoneOffset() * 60_000);
  return local.toISOString().slice(0, 16);
}

function normalizedDueValue(kind: TaskDueKind, value: string): string | null {
  if (kind === "none" || !value) return null;
  if (kind === "date") return value;
  const parsed = new Date(value);
  return Number.isNaN(parsed.getTime()) ? value : parsed.toISOString();
}

export function TaskEditorDialog({
  initial,
  sourceSubject,
  evidence,
  submitLabel = "Add Task",
  goals,
  accountId,
  goalSuggested = false,
  onClose,
  onSubmit,
}: {
  initial: TaskEditorInitial;
  /** The task account's goals; the Goal field shows once they are loaded. */
  goals?: readonly Goal[] | null;
  accountId?: string;
  /** Marks the initial goal as the assistant's suggestion for the user to confirm. */
  goalSuggested?: boolean;
  sourceSubject?: string | null;
  evidence?: string | null;
  submitLabel?: string;
  onClose(): void;
  onSubmit(values: TaskEditorValues): Promise<void> | void;
}) {
  const [title, setTitle] = useState(initial.title);
  const [notes, setNotes] = useState(initial.notes ?? "");
  const [kind, setKind] = useState<TaskKind>(initial.kind ?? "action");
  const [dueKind, setDueKind] = useState<TaskDueKind>(initial.dueKind ?? "none");
  const [dueValue, setDueValue] = useState(
    initial.dueKind === "datetime" ? dateTimeInputValue(initial.dueValue) : initial.dueValue ?? "",
  );
  const [timeZone, setTimeZone] = useState(initial.timeZone ?? Intl.DateTimeFormat().resolvedOptions().timeZone ?? "UTC");
  const [timeZoneError, setTimeZoneError] = useState<string | null>(null);
  const timeZones = useMemo(() => listSupportedTimeZones(), []);
  const [repeatIntervalDays, setRepeatIntervalDays] = useState(initial.repeatIntervalDays?.toString() ?? "");
  const [goalId, setGoalId] = useState(initial.goalId ?? "");
  const goalOptions = goals && accountId ? goalOptionsForTask(goals, { accountId, goalId: initial.goalId ?? null }) : [];
  // A suggested goal that is gone or closed by now is not offered.
  const offeredGoalId = goalOptions.some((goal) => goal.id === goalId) ? goalId : "";
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const titleRef = useRef<HTMLInputElement>(null);

  const validateTimeZone = (value: string): boolean => {
    const trimmed = value.trim();
    if (!trimmed || isValidTimeZone(trimmed)) {
      setTimeZoneError(null);
      return true;
    }
    setTimeZoneError("Choose a valid timezone, such as America/New_York.");
    return false;
  };

  const submit = async (event: React.FormEvent) => {
    event.preventDefault();
    if (!title.trim() || busy) return;
    if (dueKind !== "none" && !validateTimeZone(timeZone)) return;
    setBusy(true);
    setError(null);
    try {
      await onSubmit({
        title: title.trim(),
        notes: notes.trim() || null,
        kind,
        dueKind,
        dueValue: normalizedDueValue(dueKind, dueValue),
        timeZone: dueKind === "none" ? null : timeZone.trim() || null,
        repeatIntervalDays: repeatIntervalDays ? Number(repeatIntervalDays) : null,
        ...(goals ? { goalId: offeredGoalId || null } : {}),
      });
    } catch (reason) {
      setError(errorMessage(reason));
      setBusy(false);
    }
  };

  return (
    <Modal title={submitLabel === "Save Proposal" ? "Edit Task Proposal" : submitLabel === "Save Task" ? "Edit Task" : "Add Task"} className="task-editor-modal" onClose={onClose} initialFocusRef={titleRef}>
      <form className="modal-form" onSubmit={(event) => void submit(event)} onKeyDown={(event) => {
        if ((event.metaKey || event.ctrlKey) && event.key === "Enter") {
          event.preventDefault();
          event.currentTarget.requestSubmit();
        }
      }}>
        {sourceSubject ? <p className="modal-form-context">From: {sourceSubject}</p> : null}
        <label><span>Task</span><input ref={titleRef} value={title} onChange={(event) => setTitle(event.target.value)} placeholder="What needs doing?" /></label>
        <label><span>Type</span><select value={kind} onChange={(event) => setKind(event.target.value as TaskKind)}><option value="action">Action</option><option value="follow_up">Follow up</option><option value="waiting_for">Waiting for reply</option></select></label>
        <label><span>Due</span><select value={dueKind} onChange={(event) => {
          const nextKind = event.target.value as TaskDueKind;
          setDueKind(nextKind);
          setDueValue((current) => nextKind === "none" ? "" : convertDueInputValue(current, nextKind));
        }}><option value="none">No due date</option><option value="date">Date</option><option value="datetime">Date and time</option></select></label>
        {dueKind !== "none" ? <label><span>{dueKind === "date" ? "Due Date" : "Due Date and Time"}</span><input type={dueKind === "date" ? "date" : "datetime-local"} value={dueValue} onChange={(event) => setDueValue(event.target.value)} required /></label> : null}
        {dueKind !== "none" ? <label>
          <span>Timezone</span>
          <input
            list="task-editor-timezones"
            value={timeZone}
            aria-invalid={timeZoneError ? "true" : undefined}
            onChange={(event) => { setTimeZone(event.target.value); setTimeZoneError(null); }}
            onBlur={(event) => validateTimeZone(event.target.value)}
            placeholder="America/New_York"
          />
          <datalist id="task-editor-timezones">
            {timeZones.map((zone) => <option key={zone} value={zone} />)}
          </datalist>
        </label> : null}
        {timeZoneError ? <p className="form-error" role="alert">{timeZoneError}</p> : null}
        {kind === "follow_up" ? <label><span>Repeat Every (Days)</span><input type="number" min="1" max={MAX_REPEAT_INTERVAL_DAYS} value={repeatIntervalDays} onChange={(event) => setRepeatIntervalDays(event.target.value)} placeholder="Optional" /></label> : null}
        {goals && goalOptions.length ? <label>
          <span>Goal{goalSuggested && offeredGoalId && offeredGoalId === initial.goalId ? <small className="task-goal-suggested"> · Suggested</small> : null}</span>
          <select value={offeredGoalId} onChange={(event) => setGoalId(event.target.value)}>
            <option value="">No goal</option>
            {GOAL_HORIZONS.map((horizon) => {
              const options = goalOptions.filter((goal) => goal.horizon === horizon);
              return options.length ? <optgroup key={horizon} label={GOAL_HORIZON_LABELS[horizon]}>
                {options.map((goal) => <option key={goal.id} value={goal.id}>{goal.title} ({formatPeriod(goal.period)})</option>)}
              </optgroup> : null;
            })}
          </select>
        </label> : null}
        <label><span>Notes</span><textarea value={notes} onChange={(event) => setNotes(event.target.value)} rows={3} placeholder="Optional details" /></label>
        {evidence ? <div className="modal-form-evidence"><span>Evidence</span><blockquote>{evidence}</blockquote></div> : null}
        {error ? <p className="form-error" role="alert">{error}</p> : null}
        <div className="modal-form-actions"><button type="button" className="btn" onClick={onClose}>Cancel</button><button type="submit" className="btn btn-primary" disabled={busy || !title.trim()}>{busy ? "Saving…" : submitLabel}</button></div>
      </form>
    </Modal>
  );
}
