import { SquareCheckBig } from "lucide-react";
import { useCallback, useEffect, useState, type PointerEvent as ReactPointerEvent } from "react";
import { ContextRow, ContextSection } from "./ContextSections";
import { formatClockTime } from "./agendaEvents";
import { mailClient } from "./data/client";
import type { ScheduleEvent, ThreadTask } from "./domain";
import { errorMessage } from "./errors";
import { ICON_SIZE } from "./iconSizes";
import { bookedBlock, TASK_BLOCK_MINUTES } from "./taskBooking";
import { formatDue, isOverdue } from "./taskViews";

/** Open tasks the calendar rail shows before "Show more". */
export const OPEN_TASK_ROWS = 8;

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

/** The To Do and In Progress tasks, dragged onto the week to book time for them. */
export function CalendarTasks({ tasks, loading, error, reload, events, now, multipleAccounts, draggingTaskId, onSchedule, onDragStart }: {
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
}) {
  const rows = tasks.map((task) => {
    const due = formatDue(task);
    const booked = bookedBlock(task, events, now);
    const detail = [task.status === "in_progress" ? "In progress" : null, booked ? bookedLabel(booked) : null, multipleAccounts ? task.accountId : null]
      .filter(Boolean).join(" · ");
    return <ContextRow
      key={task.id}
      className={`calendar-task${draggingTaskId === task.id ? " calendar-task-dragging" : ""}`}
      icon={<SquareCheckBig size={ICON_SIZE.sm} />}
      title={task.title}
      wrapTitle
      date={due ? `Due ${due}` : undefined}
      dateClassName={due && isOverdue(task) ? "task-due-overdue" : undefined}
      detail={detail || undefined}
      onActivate={() => onSchedule(task)}
      onPointerDown={(event) => onDragStart(task, event)}
    />;
  });
  return <ContextSection
    id="calendar-open-tasks"
    title="Open tasks"
    count={tasks.length}
    note={tasks.length ? `Drag any task onto the calendar to block ${TASK_BLOCK_MINUTES} minutes to work on it.` : undefined}
    rows={rows}
    limit={OPEN_TASK_ROWS}
    footer={<>
      {loading && tasks.length === 0 ? <p className="context-status" role="status">Loading open tasks…</p> : null}
      {error ? <p className="context-status">Open tasks couldn't be loaded. <button type="button" className="btn-link" onClick={reload}>Retry tasks</button></p> : null}
      {!loading && !error && tasks.length === 0 ? <p className="context-status">No open tasks.</p> : null}
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
