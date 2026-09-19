import { Check, CheckSquare, Clock3, Plus, RotateCcw, X } from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";
import type { ThreadDetail, ThreadTask, TaskKind } from "./domain";
import { mailClient } from "./data/client";
import { useEscapeDismiss } from "./useEscapeDismiss";

function taskGroup(task: ThreadTask): string {
  if (task.status === "completed") return "Completed";
  if (task.kind === "waiting_for") return "Waiting for";
  if (!task.dueValue) return "No due date";
  const due = task.dueKind === "date" ? new Date(`${task.dueValue}T23:59:59`) : new Date(task.dueValue);
  const now = new Date();
  if (due.toDateString() === now.toDateString()) return "Today";
  return due.getTime() < now.getTime() ? "Today" : "Upcoming";
}

function formatDue(task: ThreadTask): string | null {
  if (!task.dueValue) return null;
  const value = task.dueKind === "date" ? new Date(`${task.dueValue}T12:00:00`) : new Date(task.dueValue);
  return value.toLocaleDateString(undefined, { month: "short", day: "numeric" });
}

export function TaskSidebar({
  onClose,
  accountId,
  currentThread,
  onOpenThread,
  onTasksChanged,
  onCheckSchedule,
  title = "Tasks",
  initialFormOpen = false,
}: {
  onClose(): void;
  accountId: string | null;
  currentThread: ThreadDetail | null;
  onOpenThread(threadId: string): void;
  onTasksChanged?(): void;
  onCheckSchedule?(): void;
  title?: string;
  initialFormOpen?: boolean;
}) {
  const [tasks, setTasks] = useState<ThreadTask[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [formOpen, setFormOpen] = useState(initialFormOpen);
  const [taskTitle, setTaskTitle] = useState("");
  const [kind, setKind] = useState<TaskKind>("action");
  const [dueValue, setDueValue] = useState("");
  const [notes, setNotes] = useState("");
  const [busy, setBusy] = useState(false);
  useEscapeDismiss(onClose);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      setTasks(await mailClient.listTasks(accountId ?? undefined));
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setLoading(false);
    }
  }, [accountId]);

  useEffect(() => { void load(); }, [load]);
  useEffect(() => {
    if (initialFormOpen && currentThread) {
      setFormOpen(true);
      if (!taskTitle) setTaskTitle(currentThread.thread.subject);
    }
  }, [currentThread, initialFormOpen, taskTitle]);

  const grouped = useMemo(() => {
    const groups = new Map<string, ThreadTask[]>();
    for (const task of tasks) {
      const group = taskGroup(task);
      const current = groups.get(group) ?? [];
      current.push(task);
      groups.set(group, current);
    }
    return ["Today", "Upcoming", "Waiting for", "No due date", "Completed"]
      .map((name) => ({ name, tasks: groups.get(name) ?? [] }))
      .filter((group) => group.tasks.length > 0);
  }, [tasks]);

  const create = async (event: React.FormEvent) => {
    event.preventDefault();
    if (!currentThread || !taskTitle.trim() || busy) return;
    setBusy(true);
    try {
      const created = await mailClient.createTask({
        accountId: currentThread.thread.accountId,
        threadId: currentThread.thread.id,
        sourceMessageId: currentThread.messages.at(-1)?.id ?? null,
        subjectSnapshot: currentThread.thread.subject,
        title: taskTitle,
        notes: notes || null,
        kind,
        dueKind: dueValue ? "date" : "none",
        dueValue: dueValue || null,
        timeZone: Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC",
        evidenceText: currentThread.messages.at(-1)?.bodyText.slice(0, 1000) ?? null,
      });
      setTasks((current) => [created, ...current]);
      setTaskTitle("");
      setNotes("");
      setDueValue("");
      setFormOpen(false);
      onTasksChanged?.();
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  };

  const setStatus = async (task: ThreadTask, status: "open" | "completed") => {
    try {
      const updated = await mailClient.setTaskStatus(task.id, status);
      setTasks((current) => current.map((candidate) => candidate.id === updated.id ? updated : candidate));
      onTasksChanged?.();
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    }
  };

  return (
    <aside className="tasks-sidebar" aria-label={title}>
      <header className="tasks-sidebar-header">
        <h2><CheckSquare size={18} /> {title}</h2>
        <div>
          {onCheckSchedule ? <button type="button" aria-label="Check schedule" title="Check schedule" onClick={onCheckSchedule}><Clock3 size={17} /></button> : null}
          {currentThread ? <button type="button" aria-label="Open add task form" title="Add task" onClick={() => setFormOpen((open) => !open)}><Plus size={17} /></button> : null}
          <button type="button" aria-label="Close tasks" onClick={onClose}><X size={18} /></button>
        </div>
      </header>
      {formOpen && currentThread ? (
        <form className="task-create-form" onSubmit={(event) => void create(event)}>
          <label><span>Task</span><input autoFocus value={taskTitle} onChange={(event) => setTaskTitle(event.target.value)} placeholder="What needs doing?" /></label>
          <label><span>Type</span><select value={kind} onChange={(event) => setKind(event.target.value as TaskKind)}><option value="action">Action</option><option value="follow_up">Follow up</option><option value="waiting_for">Waiting for reply</option></select></label>
          <label><span>Due date</span><input type="date" value={dueValue} onChange={(event) => setDueValue(event.target.value)} /></label>
          <label><span>Notes</span><textarea value={notes} onChange={(event) => setNotes(event.target.value)} rows={2} placeholder="Optional details" /></label>
          <div className="task-create-actions"><button type="submit" disabled={busy || !taskTitle.trim()}>Add task</button><button type="button" onClick={() => setFormOpen(false)}>Cancel</button></div>
        </form>
      ) : null}
      {error ? <p className="tasks-error" role="alert">{error}</p> : null}
      {loading ? <p className="tasks-status">Loading tasks…</p> : null}
      {!loading && grouped.length === 0 ? <p className="tasks-status">No tasks yet. Add one from a conversation.</p> : null}
      <div className="tasks-list">
        {grouped.map((group) => (
          <section key={group.name} aria-labelledby={`task-group-${group.name.replace(/\s/g, "-")}`}>
            <h3 id={`task-group-${group.name.replace(/\s/g, "-")}`}>{group.name}</h3>
            {group.tasks.map((task) => (
              <article className={`task-card task-${task.status}`} key={task.id}>
                <button type="button" className="task-card-main" onClick={() => onOpenThread(task.threadId)}>
                  <strong>{task.title}</strong>
                  <span>{task.subjectSnapshot}</span>
                  {formatDue(task) ? <small><Clock3 size={12} /> {formatDue(task)}</small> : null}
                </button>
                <button type="button" className="task-status-button" aria-label={task.status === "completed" ? `Reopen ${task.title}` : `Complete ${task.title}`} onClick={() => void setStatus(task, task.status === "completed" ? "open" : "completed")}>
                  {task.status === "completed" ? <RotateCcw size={15} /> : <Check size={15} />}
                </button>
              </article>
            ))}
          </section>
        ))}
      </div>
    </aside>
  );
}
