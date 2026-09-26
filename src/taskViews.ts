import type { ThreadTask } from "./domain";

export const TASK_VIEWS = ["All", "Overdue", "Today", "Upcoming", "Anytime", "Waiting", "Completed"] as const;
export type TaskView = typeof TASK_VIEWS[number];
type OpenTaskView = Exclude<TaskView, "All" | "Completed">;

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
  if (task.status !== "open") return "Completed";
  const due = dueView(task, now);
  if (due) return due;
  return task.kind === "waiting_for" ? "Waiting" : "Anytime";
}

export function taskMatchesView(task: ThreadTask, view: TaskView, now: Date): boolean {
  if (view === "Completed") return task.status !== "open";
  if (task.status !== "open") return false;
  if (view === "All") return true;
  if (view === "Waiting") return task.kind === "waiting_for";
  if (view === "Anytime") return !dueView(task, now) && task.kind !== "waiting_for";
  return dueView(task, now) === view;
}
