import { MessageSquare } from "lucide-react";
import { useMemo, useRef, useState } from "react";
import { Modal } from "./AppChrome";
import { convertDueInputValue, isValidTimeZone, listSupportedTimeZones } from "./calendarTime";
import { isEditableTarget } from "./commands";
import type { Goal, TaskDueKind, TaskKind, TaskStatus, ThreadTask, UpdateTaskRequest } from "./domain";
import { errorMessage } from "./errors";
import { formatPeriod, GOAL_HORIZON_LABELS, GOAL_HORIZONS, goalOptionsForTask } from "./goals";

/** The longest repeat interval the task store accepts. */
export const MAX_REPEAT_INTERVAL_DAYS = 365;

export type TaskDetailDrafts = {
  title: string;
  notes: string;
  kind: TaskKind;
  dueKind: TaskDueKind;
  dueValue: string;
  timeZone: string;
  repeatIntervalDays: string;
  /** "" for no goal. */
  goalId: string;
};

function localTimeZone(): string {
  return Intl.DateTimeFormat().resolvedOptions().timeZone ?? "UTC";
}

function dateTimeInputValue(value: string | null | undefined): string {
  if (!value) return "";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value.slice(0, 16);
  return new Date(date.getTime() - date.getTimezoneOffset() * 60_000).toISOString().slice(0, 16);
}

export function taskDetailDrafts(task: ThreadTask): TaskDetailDrafts {
  return {
    title: task.title,
    notes: task.notes ?? "",
    kind: task.kind,
    dueKind: task.dueKind,
    dueValue: task.dueKind === "datetime" ? dateTimeInputValue(task.dueValue) : task.dueValue ?? "",
    timeZone: task.timeZone ?? localTimeZone(),
    repeatIntervalDays: task.repeatIntervalDays?.toString() ?? "",
    goalId: task.goalId ?? "",
  };
}

/**
 * The saved fields that differ from the task, plus messages for fields that
 * cannot be saved yet. Invalid or incomplete fields are left out of the
 * request so every valid edit still saves.
 */
export function taskDetailChanges(task: ThreadTask, drafts: TaskDetailDrafts): { request: Omit<UpdateTaskRequest, "id">; errors: string[] } {
  const request: Omit<UpdateTaskRequest, "id"> = {};
  const errors: string[] = [];

  const title = drafts.title.trim();
  if (!title) errors.push("A task needs a title.");
  else if (title !== task.title) request.title = title;

  const notes = drafts.notes.trim() || null;
  if (notes !== (task.notes ?? null)) request.notes = notes;

  if (drafts.kind !== task.kind) request.kind = drafts.kind;
  if ((drafts.goalId || null) !== (task.goalId ?? null)) request.goalId = drafts.goalId || null;

  if (drafts.kind === "follow_up") {
    const raw = drafts.repeatIntervalDays.trim();
    const repeat = raw ? Number(raw) : null;
    if (repeat !== null && (!Number.isInteger(repeat) || repeat < 1 || repeat > MAX_REPEAT_INTERVAL_DAYS)) {
      errors.push(`Repeat every 1 to ${MAX_REPEAT_INTERVAL_DAYS} days.`);
    } else if (repeat !== (task.repeatIntervalDays ?? null)) {
      request.repeatIntervalDays = repeat;
    }
  }

  if (drafts.dueKind === "none") {
    if (task.dueKind !== "none") Object.assign(request, { dueKind: "none", dueValue: null, timeZone: null });
  } else if (drafts.dueValue) {
    const zone = drafts.timeZone.trim() || localTimeZone();
    if (drafts.dueKind === "datetime" && !isValidTimeZone(zone)) {
      errors.push("Choose a valid timezone, such as America/New_York.");
    } else {
      const parsed = new Date(drafts.dueValue);
      const dueValue = drafts.dueKind === "date" || Number.isNaN(parsed.getTime()) ? drafts.dueValue : parsed.toISOString();
      const timeZone = drafts.dueKind === "datetime" ? zone : null;
      if (drafts.dueKind !== task.dueKind || dueValue !== (task.dueValue ?? null) || timeZone !== (task.timeZone ?? null)) {
        Object.assign(request, { dueKind: drafts.dueKind, dueValue, timeZone });
      }
    }
  }

  return { request, errors };
}

const STATUS_OPTIONS: { value: TaskStatus; label: string }[] = [
  { value: "open", label: "To Do" },
  { value: "in_progress", label: "In Progress" },
  { value: "completed", label: "Done" },
];

/**
 * Every field of one task, saved as the user leaves it. Escape and Cmd+Enter
 * save what is pending and close; outside a field, j/k step to the adjacent
 * task and o opens the source conversation.
 */
export function TaskDetailDialog({
  task,
  goals = [],
  position,
  onUpdate,
  onSetStatus,
  onNavigate,
  onOpenThread,
  onClose,
}: {
  task: ThreadTask;
  /** Goals the task may support; only active goals in its account are offered. */
  goals?: readonly Goal[];
  position?: { index: number; total: number } | null;
  onUpdate(request: Omit<UpdateTaskRequest, "id">): Promise<void>;
  onSetStatus(status: TaskStatus): void;
  onNavigate(direction: -1 | 1): void;
  onOpenThread(threadId: string): void;
  onClose(): void;
}) {
  const [drafts, setDrafts] = useState(() => taskDetailDrafts(task));
  const [errors, setErrors] = useState<string[]>([]);
  const [saveError, setSaveError] = useState<string | null>(null);
  const draftsRef = useRef(drafts);
  draftsRef.current = drafts;
  const bodyRef = useRef<HTMLDivElement>(null);
  const timeZones = useMemo(() => listSupportedTimeZones(), []);

  // Reset in the render that changes tasks so the old task's drafts never diff against the new one.
  const [draftTaskId, setDraftTaskId] = useState(task.id);
  if (draftTaskId !== task.id) {
    setDraftTaskId(task.id);
    setDrafts(taskDetailDrafts(task));
    setErrors([]);
    setSaveError(null);
  }

  const commit = (next: TaskDetailDrafts = draftsRef.current) => {
    const { request, errors: nextErrors } = taskDetailChanges(task, next);
    setErrors(nextErrors);
    if (Object.keys(request).length === 0) return;
    setSaveError(null);
    onUpdate(request).catch((reason) => setSaveError(errorMessage(reason)));
  };

  const change = <K extends keyof TaskDetailDrafts>(field: K, value: TaskDetailDrafts[K], save = false) => {
    const next = { ...draftsRef.current, [field]: value };
    draftsRef.current = next;
    setDrafts(next);
    if (save) commit(next);
  };

  const close = () => {
    commit();
    onClose();
  };

  const navigate = (direction: -1 | 1) => {
    commit();
    onNavigate(direction);
  };

  const onKeyDown = (event: React.KeyboardEvent) => {
    if ((event.metaKey || event.ctrlKey) && event.key === "Enter") {
      event.preventDefault();
      close();
      return;
    }
    if (event.key === "Enter" && event.target instanceof HTMLInputElement) {
      event.preventDefault();
      commit();
      return;
    }
    if (event.metaKey || event.ctrlKey || event.altKey || isEditableTarget(event.target)) return;
    if (event.key === "j" || event.key === "ArrowDown") {
      event.preventDefault();
      navigate(1);
    } else if (event.key === "k" || event.key === "ArrowUp") {
      event.preventDefault();
      navigate(-1);
    } else if (event.key === "o" && task.threadId) {
      event.preventDefault();
      close();
      onOpenThread(task.threadId);
    }
  };

  const goalOptions = goalOptionsForTask(goals, task);
  const messages = [...errors, ...(saveError ? [saveError] : [])];
  const statusOptions = task.status === "cancelled" ? [...STATUS_OPTIONS, { value: "cancelled" as const, label: "Cancelled" }] : STATUS_OPTIONS;

  return (
    <Modal title="Task details" className="task-editor-modal task-detail-modal" onClose={close} initialFocusRef={bodyRef}>
      <div ref={bodyRef} className="modal-form task-detail-form" tabIndex={-1} onKeyDown={onKeyDown}>
        <label className="task-detail-title-field">
          <span>Title</span>
          <input
            value={drafts.title}
            onChange={(event) => change("title", event.target.value)}
            onBlur={() => commit()}
            maxLength={240}
            aria-invalid={drafts.title.trim() ? undefined : "true"}
          />
        </label>
        <div className="task-detail-row">
          <label><span>Status</span><select value={task.status} onChange={(event) => onSetStatus(event.target.value as TaskStatus)}>
            {statusOptions.map((option) => <option key={option.value} value={option.value}>{option.label}</option>)}
          </select></label>
          <label><span>Type</span><select value={drafts.kind} onChange={(event) => change("kind", event.target.value as TaskKind, true)}>
            <option value="action">Action</option><option value="follow_up">Follow up</option><option value="waiting_for">Waiting for reply</option>
          </select></label>
        </div>
        <label><span>Goal</span><select value={drafts.goalId} onChange={(event) => change("goalId", event.target.value, true)}>
          <option value="">No goal</option>
          {GOAL_HORIZONS.map((horizon) => {
            const options = goalOptions.filter((goal) => goal.horizon === horizon);
            return options.length ? <optgroup key={horizon} label={GOAL_HORIZON_LABELS[horizon]}>
              {options.map((goal) => <option key={goal.id} value={goal.id}>{goal.title} ({formatPeriod(goal.period)})</option>)}
            </optgroup> : null;
          })}
        </select></label>
        <div className="task-detail-row">
          <label><span>Due</span><select value={drafts.dueKind} onChange={(event) => {
            const nextKind = event.target.value as TaskDueKind;
            const next = { ...draftsRef.current, dueKind: nextKind, dueValue: nextKind === "none" ? "" : convertDueInputValue(draftsRef.current.dueValue, nextKind) };
            draftsRef.current = next;
            setDrafts(next);
            commit(next);
          }}><option value="none">No due date</option><option value="date">Date</option><option value="datetime">Date and time</option></select></label>
          {drafts.dueKind !== "none" ? <label>
            <span>{drafts.dueKind === "date" ? "Due date" : "Due date and time"}</span>
            <input type={drafts.dueKind === "date" ? "date" : "datetime-local"} value={drafts.dueValue} onChange={(event) => change("dueValue", event.target.value)} onBlur={() => commit()} />
          </label> : null}
        </div>
        {drafts.dueKind === "datetime" ? <label>
          <span>Timezone</span>
          <input list="task-detail-timezones" value={drafts.timeZone} onChange={(event) => change("timeZone", event.target.value)} onBlur={() => commit()} placeholder="America/New_York" />
          <datalist id="task-detail-timezones">{timeZones.map((zone) => <option key={zone} value={zone} />)}</datalist>
        </label> : null}
        {drafts.kind === "follow_up" ? <label>
          <span>Repeat every (days)</span>
          <input type="number" min="1" max={MAX_REPEAT_INTERVAL_DAYS} value={drafts.repeatIntervalDays} onChange={(event) => change("repeatIntervalDays", event.target.value)} onBlur={() => commit()} placeholder="Optional" />
        </label> : null}
        <label>
          <span>Description</span>
          <textarea value={drafts.notes} onChange={(event) => change("notes", event.target.value)} onBlur={() => commit()} rows={5} maxLength={8000} placeholder="Add a description" />
        </label>
        {messages.map((message) => <p key={message} className="form-error" role="alert">{message}</p>)}
        {task.threadId ? <section className="task-detail-source" aria-label="Source conversation">
          <h3>Source conversation</h3>
          {task.subjectSnapshot ? <p>{task.subjectSnapshot}</p> : null}
          <button type="button" className="btn-link" onClick={() => { close(); onOpenThread(task.threadId!); }}><MessageSquare size={16} /> Open conversation</button>
          {task.evidenceText ? <details><summary>Source excerpt</summary><blockquote>{task.evidenceText}</blockquote></details> : null}
        </section> : null}
        <footer className="task-detail-hints">
          <span>Changes save automatically</span>
          <span><kbd>j</kbd>/<kbd>k</kbd> {position ? `${position.index + 1} of ${position.total}` : "next/previous"}{task.threadId ? <> · <kbd>o</kbd> open conversation</> : null} · <kbd>Esc</kbd> close</span>
        </footer>
      </div>
    </Modal>
  );
}
