import { useCallback, useEffect, useRef, useState, type PointerEvent as ReactPointerEvent } from "react";
import { HoverTooltip } from "./AppChrome";
import { ContextRow, ContextSection } from "./ContextSections";
import { formatClockTime } from "./agendaEvents";
import { mailClient } from "./data/client";
import type { ScheduleEvent, ThreadTask } from "./domain";
import { errorMessage } from "./errors";
import { bookedBlock, TASK_BLOCK_MINUTES } from "./taskBooking";
import { formatDue, isOverdue } from "./taskViews";
import type { Notice } from "./useNotice";

/** Open tasks the calendar rail shows before "Show more". */
export const OPEN_TASK_ROWS = 8;

/** How long a task checked off in Open tasks stays, struck through, before it leaves the list. */
export const DONE_TASK_LINGER_MS = 4000;

/** A task checked off (or back on) here, shown as such until the reloaded list agrees. */
type Checked = { task: ThreadTask; previous: ThreadTask["status"]; index: number; done: boolean };

/** Every task of these accounts, reloaded whenever `refreshKey` changes. */
export function useAccountTasks(accountIds: string[], refreshKey: number) {
  const [tasks, setTasks] = useState<ThreadTask[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);
  const scope = accountIds.join("\n");
  useEffect(() => {
    let current = true;
    const ids = scope ? scope.split("\n") : [];
    setLoading(true);
    setError(null);
    mailClient.listTasks(ids.length === 1 ? ids[0] : undefined)
      .then((loaded) => { if (current) setTasks(loaded.filter((task) => ids.includes(task.accountId))); })
      .catch((reason: unknown) => { if (current) setError(errorMessage(reason)); })
      .finally(() => { if (current) setLoading(false); });
    return () => { current = false; };
  }, [scope, refreshKey, attempt]);
  const reload = useCallback(() => setAttempt((value) => value + 1), []);
  return { tasks, loading, error, reload };
}

function bookedLabel(event: ScheduleEvent): string {
  const start = new Date(event.start);
  return `Booked ${new Intl.DateTimeFormat(undefined, { weekday: "short" }).format(start)} ${formatClockTime(start)}`;
}

/** The To Do and In Progress tasks, dragged onto the week to book time for them or checked off. */
export function CalendarTasks({ tasks, loading, error, reload, events, now, multipleAccounts, draggingTaskId, onSchedule, onDragStart, onChanged, onNotice }: {
  /** Open tasks, in board order. */
  tasks: ThreadTask[];
  loading: boolean;
  error: string | null;
  reload(): void;
  /** The loaded schedule, to say which tasks already have time booked. */
  events: ScheduleEvent[];
  now: Date;
  multipleAccounts: boolean;
  draggingTaskId?: string | null;
  /** Opens New Event for the task: the keyboard way to book time. */
  onSchedule(task: ThreadTask): void;
  onDragStart(task: ThreadTask, event: ReactPointerEvent<HTMLElement>): void;
  /** A task's status changed here. */
  onChanged(): void;
  /** Says a task was completed, with Undo, or why it couldn't be. */
  onNotice?(notice: Notice): void;
}) {
  const [checked, setChecked] = useState<Checked[]>([]);
  const lingering = useRef(new Map<string, number>());
  useEffect(() => () => { for (const timer of lingering.current.values()) window.clearTimeout(timer); }, []);

  // A task reopened here goes back to the reloaded list's own row once that list has it again.
  useEffect(() => {
    setChecked((current) => {
      const next = current.filter((entry) => entry.done || !tasks.some((task) => task.id === entry.task.id));
      return next.length === current.length ? current : next;
    });
  }, [tasks]);

  const stopLingering = (id: string) => {
    window.clearTimeout(lingering.current.get(id));
    lingering.current.delete(id);
  };
  const put = (entry: Checked) => setChecked((current) => [...current.filter((candidate) => candidate.task.id !== entry.task.id), entry]);
  const forget = (id: string) => setChecked((current) => current.filter((entry) => entry.task.id !== id));

  const reopen = async (entry: Checked) => {
    stopLingering(entry.task.id);
    put({ ...entry, done: false });
    try {
      await mailClient.setTaskStatus(entry.task.id, entry.previous);
    } catch (reason) {
      forget(entry.task.id);
      onNotice?.({ message: errorMessage(reason) });
      return;
    }
    onChanged();
  };

  // The checked-off row stays a moment, struck through, so a slip of the mouse can be unchecked.
  const complete = async (task: ThreadTask, index: number) => {
    const entry: Checked = { task, previous: task.status, index, done: true };
    put(entry);
    try {
      await mailClient.setTaskStatus(task.id, "completed");
    } catch (reason) {
      forget(task.id);
      onNotice?.({ message: errorMessage(reason) });
      return;
    }
    onChanged();
    stopLingering(task.id);
    lingering.current.set(task.id, window.setTimeout(() => {
      lingering.current.delete(task.id);
      forget(task.id);
    }, DONE_TASK_LINGER_MS));
    onNotice?.({ message: `Completed: ${task.title}`, undo: () => void reopen(entry) });
  };

  // Rows keep their place while checked, even after the reloaded list has dropped them.
  const shown: { task: ThreadTask; entry?: Checked }[] = tasks.map((task) => ({ task, entry: checked.find((entry) => entry.task.id === task.id) }));
  for (const entry of [...checked].sort((a, b) => a.index - b.index)) {
    if (!tasks.some((task) => task.id === entry.task.id)) shown.splice(Math.min(entry.index, shown.length), 0, { task: entry.task, entry });
  }

  const rows = shown.map(({ task, entry }, index) => {
    const done = entry?.done ?? false;
    const due = formatDue(task);
    const booked = bookedBlock(task, events, now);
    const detail = [task.status === "in_progress" ? "In progress" : null, booked ? bookedLabel(booked) : null, multipleAccounts ? task.accountId : null]
      .filter(Boolean).join(" · ");
    const classes = ["calendar-task", done ? "context-task-done" : "", draggingTaskId === task.id ? "calendar-task-dragging" : ""].filter(Boolean).join(" ");
    return <ContextRow
      key={task.id}
      className={classes}
      control={<HoverTooltip label={done ? "Mark not done" : "Mark done"}>
        <input
          type="checkbox"
          className="context-task-checkbox"
          checked={done}
          aria-label={done ? `Mark ${task.title} not done` : `Mark ${task.title} done`}
          onChange={() => void (entry?.done ? reopen(entry) : complete(task, index))}
        />
      </HoverTooltip>}
      title={task.title}
      wrapTitle
      date={due ? `Due ${due}` : undefined}
      dateClassName={due && !done && isOverdue(task) ? "task-due-overdue" : undefined}
      detail={detail || undefined}
      onActivate={done ? undefined : () => onSchedule(task)}
      // A press on the checkbox only ticks it; the rest of the row drags.
      onPointerDown={done ? undefined : (event) => {
        if (!(event.target instanceof Element && event.target.closest(".context-row-control"))) onDragStart(task, event);
      }}
    />;
  });
  const openCount = shown.filter(({ entry }) => !entry?.done).length;
  return <ContextSection
    id="calendar-open-tasks"
    title="Open tasks"
    count={openCount}
    note={openCount ? `Drag any task onto the calendar to block ${TASK_BLOCK_MINUTES} minutes to work on it.` : undefined}
    rows={rows}
    limit={OPEN_TASK_ROWS}
    footer={<>
      {loading && shown.length === 0 ? <p className="context-status" role="status">Loading open tasks…</p> : null}
      {error ? <p className="context-status">Open tasks couldn't be loaded. <button type="button" className="btn-link" onClick={reload}>Retry tasks</button></p> : null}
      {!loading && !error && shown.length === 0 ? <p className="context-status">No open tasks.</p> : null}
    </>}
  />;
}

/** The task an event books time for, in its details, so it can be ticked off there. */
export function BookedTask({ task, onChanged }: { task: ThreadTask; onChanged(): void }) {
  const [error, setError] = useState<string | null>(null);
  const done = task.status === "completed";
  const toggle = async () => {
    setError(null);
    try {
      await mailClient.setTaskStatus(task.id, done ? "open" : "completed");
      onChanged();
    } catch (reason) {
      setError(errorMessage(reason));
    }
  };
  const due = formatDue(task);
  return <ContextSection
    id="calendar-booked-task"
    className="context-tasks"
    title="Task"
    rows={[<ContextRow
      as="article"
      key={task.id}
      className={done ? "context-task context-task-done" : "context-task"}
      control={<input
        type="checkbox"
        className="context-task-checkbox"
        checked={done}
        aria-label={done ? `Mark ${task.title} not done` : `Mark ${task.title} done`}
        onChange={() => void toggle()}
      />}
      title={task.title}
      wrapTitle
      date={due ? `Due ${due}` : undefined}
      dateClassName={due && !done && isOverdue(task) ? "task-due-overdue" : undefined}
    />]}
    footer={error ? <p className="form-error" role="alert">{error}</p> : undefined}
  />;
}
