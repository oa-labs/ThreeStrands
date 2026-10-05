import type { TaskStatus, ThreadTask } from "./domain";

export const TASK_VIEWS = ["All", "Overdue", "Today", "Upcoming", "Anytime", "Waiting", "Completed"] as const;
export type TaskView = typeof TASK_VIEWS[number];
type OpenTaskView = Exclude<TaskView, "All" | "Completed">;

export function isActiveTaskStatus(status: TaskStatus): boolean {
  return status === "open" || status === "in_progress";
}

export const TASK_BOARD_COLUMNS = ["To Do", "In Progress", "Done"] as const;
export type TaskBoardColumn = typeof TASK_BOARD_COLUMNS[number];

export function taskBoardColumn(status: TaskStatus): TaskBoardColumn {
  if (status === "open") return "To Do";
  return status === "in_progress" ? "In Progress" : "Done";
}

const COLUMN_STATUS: Record<TaskBoardColumn, TaskStatus> = { "To Do": "open", "In Progress": "in_progress", Done: "completed" };

/** The status a task takes when moved one board column left or right, or null at the edge. */
export function adjacentTaskStatus(status: TaskStatus, direction: -1 | 1): TaskStatus | null {
  const column = TASK_BOARD_COLUMNS[TASK_BOARD_COLUMNS.indexOf(taskBoardColumn(status)) + direction];
  return column ? COLUMN_STATUS[column] : null;
}

/** The status a task takes when dropped on a board column. */
export function taskBoardColumnStatus(column: TaskBoardColumn): TaskStatus {
  return COLUMN_STATUS[column];
}

function dueTime(task: ThreadTask): number | null {
  if (!task.dueValue || task.dueKind === "none") return null;
  // A date-only task stays due through the end of its day.
  const time = new Date(task.dueKind === "date" ? `${task.dueValue}T23:59:59` : task.dueValue).getTime();
  return Number.isNaN(time) ? null : time;
}

function timestamp(value: string | null | undefined): number {
  const time = value ? new Date(value).getTime() : Number.NaN;
  return Number.isNaN(time) ? 0 : time;
}

/**
 * Orders tasks within a board column or list group: active work by soonest due
 * (so overdue rises to the top) with undated tasks after in creation order;
 * finished work by most recently completed.
 */
export function compareTasksForDisplay(a: ThreadTask, b: ThreadTask): number {
  const aActive = isActiveTaskStatus(a.status);
  const bActive = isActiveTaskStatus(b.status);
  if (aActive !== bActive) return aActive ? -1 : 1;
  if (!aActive) return timestamp(b.completedAt ?? b.updatedAt) - timestamp(a.completedAt ?? a.updatedAt);
  const aDue = dueTime(a);
  const bDue = dueTime(b);
  if (aDue !== bDue) {
    if (aDue === null) return 1;
    if (bDue === null) return -1;
    return aDue - bDue;
  }
  return timestamp(a.createdAt) - timestamp(b.createdAt);
}

function localDateKey(date: Date): string {
  const year = date.getFullYear();
  const month = String(date.getMonth() + 1).padStart(2, "0");
  const day = String(date.getDate()).padStart(2, "0");
  return `${year}-${month}-${day}`;
}

export function dueView(task: ThreadTask, now: Date): "Overdue" | "Today" | "Upcoming" | null {
  if (!task.dueValue || task.dueKind === "none") return null;
  if (task.dueKind === "date") {
    const today = localDateKey(now);
    if (task.dueValue < today) return "Overdue";
    return task.dueValue === today ? "Today" : "Upcoming";
  }
  const due = new Date(task.dueValue);
  if (Number.isNaN(due.getTime())) return null;
  if (due.getTime() < now.getTime()) return "Overdue";
  return localDateKey(due) === localDateKey(now) ? "Today" : "Upcoming";
}

export function taskViewForAll(task: ThreadTask, now: Date): OpenTaskView | "Completed" {
  if (!isActiveTaskStatus(task.status)) return "Completed";
  const due = dueView(task, now);
  if (due) return due;
  return task.kind === "waiting_for" ? "Waiting" : "Anytime";
}

export function taskMatchesView(task: ThreadTask, view: TaskView, now: Date): boolean {
  if (view === "Completed") return !isActiveTaskStatus(task.status);
  if (!isActiveTaskStatus(task.status)) return false;
  if (view === "All") return true;
  if (view === "Waiting") return task.kind === "waiting_for";
  if (view === "Anytime") return !dueView(task, now) && task.kind !== "waiting_for";
  return dueView(task, now) === view;
}

export function isOverdue(task: ThreadTask): boolean {
  return isActiveTaskStatus(task.status) && dueView(task, new Date()) === "Overdue";
}

/** A task marked done on `now`'s local calendar day, still worth showing so it can be unchecked. */
export function isCompletedToday(task: ThreadTask, now: Date = new Date()): boolean {
  if (task.status !== "completed" || !task.completedAt) return false;
  const completed = new Date(task.completedAt);
  return completed.getFullYear() === now.getFullYear()
    && completed.getMonth() === now.getMonth()
    && completed.getDate() === now.getDate();
}

export function formatRelativeDate(date: Date, now: Date = new Date()): string {
  const startOfDay = (value: Date) => new Date(value.getFullYear(), value.getMonth(), value.getDate()).getTime();
  const diffDays = Math.round((startOfDay(date) - startOfDay(now)) / 86_400_000);
  if (diffDays === 0) return "Today";
  if (diffDays === 1) return "Tomorrow";
  if (diffDays === -1) return "Yesterday";
  const includeYear = date.getFullYear() !== now.getFullYear();
  return date.toLocaleDateString(undefined, includeYear ? { year: "numeric", month: "short", day: "numeric" } : { month: "short", day: "numeric" });
}

export function formatDue(task: ThreadTask): string | null {
  if (!task.dueValue) return null;
  const value = task.dueKind === "date" ? new Date(`${task.dueValue}T12:00:00`) : new Date(task.dueValue);
  return formatRelativeDate(value);
}

export function isDue(task: ThreadTask): boolean {
  if (!isActiveTaskStatus(task.status) || !task.dueValue) return false;
  const due = task.dueKind === "date" ? new Date(`${task.dueValue}T23:59:59`) : new Date(task.dueValue);
  return due.getTime() <= Date.now();
}
