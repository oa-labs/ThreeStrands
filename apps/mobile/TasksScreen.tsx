import { useState, type FormEvent } from "react";
import type { ThreadTask } from "../../src/domain";
import { noteTaskAsTask, type NoteTaskRecord } from "../../src/noteEntities";
import { formatDue } from "../../src/taskViews";
import { errorText, mobile, type TaskEdit } from "./api";
import { useResource } from "./useResource";

export function TasksScreen({ revision, onChange, onOpenNote, onNew, onEdit, done, setDone }: {
  revision: number;
  onChange(): void;
  onOpenNote(task: NoteTaskRecord): void;
  onNew(): void;
  onEdit(id: string): void;
  done: boolean;
  setDone(value: boolean): void;
}) {
  const [retry, setRetry] = useState(0);
  const { data, error, setData } = useResource(mobile.tasks, [revision, retry]);
  const [pending, setPending] = useState<Set<string>>(() => new Set());
  const [failure, setFailure] = useState<string>();
  const tasks = [...(data?.standalone ?? []), ...(data?.fromNotes.map(noteTaskAsTask) ?? [])]
    .filter(task => done ? task.status === "completed" : task.status === "open" || task.status === "in_progress");

  async function check(task: ThreadTask) {
    const checked = task.status !== "completed";
    setPending(current => new Set(current).add(task.id));
    setFailure(undefined);
    try {
      if (task.noteSource) {
        const source = task.noteSource;
        await mobile.checkNoteItem(source.noteId, source.markerId, source.updatedAt, checked);
        setData(current => current && { ...current, fromNotes: current.fromNotes.map(item => item.id === task.id ? { ...item, checked } : item) });
      } else {
        const saved = await mobile.checkTask(task.id, checked);
        setData(current => current && { ...current, standalone: current.standalone.map(item => item.id === saved.id ? saved : item) });
      }
      onChange();
    } catch (error) {
      setFailure(errorText(error));
      setRetry(value => value + 1);
    } finally {
      setPending(current => {
        const next = new Set(current);
        next.delete(task.id);
        return next;
      });
    }
  }

  return <section aria-label="Tasks">
    <div className="section-heading"><h1>Tasks</h1><button onClick={onNew}>New Task</button></div>
    <div className="segments" aria-label="Task views">
      <button aria-pressed={!done} onClick={() => setDone(false)}>Open</button>
      <button aria-pressed={done} onClick={() => setDone(true)}>Completed</button>
    </div>
    {(error || failure) && <p role="alert">{failure || error} <button onClick={() => setRetry(value => value + 1)}>Refresh</button></p>}
    {!data && !error && <p role="status">Loading tasks…</p>}
    {data && !tasks.length && <p className="muted">{done ? "No completed tasks." : "No open tasks."}</p>}
    <ul className="rows">
      {tasks.map(task => {
        const context = task.noteSource?.noteTitle ?? task.subjectSnapshot;
        const due = formatDue(task);
        return <li key={task.id} className="task-row">
          <button type="button" className="note-check round" role="checkbox" aria-checked={task.status === "completed"}
            aria-label={task.title} disabled={pending.has(task.id) || task.noteSource?.conflicted}
            onClick={() => void check(task)}>{task.status === "completed" ? "✓" : ""}</button>
          <button className="row-button" onClick={() => {
            if (task.noteSource) onOpenNote(task.noteSource);
            else onEdit(task.id);
          }}>
            <strong>{task.title}</strong>
            {(context || due) && <span>{[context, due].filter(Boolean).join(" · ")}</span>}
            {task.noteSource?.conflicted && <span>Review the note conflict to complete this task.</span>}
          </button>
        </li>;
      })}
    </ul>
  </section>;
}

export function EditTask({ id, onSaved, onCancel }: { id: string; onSaved(): void; onCancel(): void }) {
  const { data, error } = useResource(mobile.tasks, [id]);
  const task = data?.standalone.find(row => row.id === id);
  if (error) return <p role="alert">{error}</p>;
  if (!data) return <p role="status">Loading task…</p>;
  if (!task) return <p>This task is unavailable.</p>;
  return <TaskForm task={task} onSaved={onSaved} onCancel={onCancel} />;
}

export function TaskForm({ task, onSaved, onCancel }: { task?: ThreadTask; onSaved(): void; onCancel(): void }) {
  const [title, setTitle] = useState(task?.title ?? "");
  const [notes, setNotes] = useState(task?.notes ?? "");
  const [dueDate, setDueDate] = useState(task?.dueKind === "date" ? task.dueValue ?? "" : "");
  const [dateChanged, setDateChanged] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string>();
  const existingDue = task?.dueKind === "datetime" ? formatDue(task) : null;
  async function submit(event: FormEvent) {
    event.preventDefault();
    setSaving(true);
    try {
      if (task) {
        const edit: TaskEdit = { id: task.id };
        if (title !== task.title) edit.title = title;
        if (notes !== (task.notes ?? "")) edit.notes = notes || null;
        if (dateChanged) {
          edit.dueKind = dueDate ? "date" : "none";
          edit.dueValue = dueDate || null;
          edit.timeZone = null;
        }
        if (Object.keys(edit).length > 1) await mobile.updateTask(edit);
      } else {
        await mobile.createTask(title, notes || null, dueDate || null);
      }
      onSaved();
    } catch (failure) { setError(errorText(failure)); }
    finally { setSaving(false); }
  }

  return <form className="editor" onSubmit={submit}>
    <h1>{task ? "Task details" : "New Task"}</h1>
    {task?.subjectSnapshot && <p className="muted">Linked to “{task.subjectSnapshot}” on desktop.</p>}
    <label>Title<input required autoFocus={!task} value={title} onChange={event => setTitle(event.target.value)} /></label>
    <label>Notes<textarea value={notes} onChange={event => setNotes(event.target.value)} /></label>
    {existingDue && !dateChanged && <p className="muted">Due {existingDue}</p>}
    <label>{existingDue ? "Change due date" : "Due date"}<input type="date" value={dueDate} onChange={event => {
      setDueDate(event.target.value);
      setDateChanged(true);
    }} /></label>
    {existingDue && !dateChanged && <button type="button" onClick={() => setDateChanged(true)}>Clear due date</button>}
    {error && <p role="alert">{error}</p>}
    <div className="note-actions">
      <button className="primary" disabled={saving || !title.trim()}>{task ? "Save Changes" : "Add Task"}</button>
      <button type="button" onClick={onCancel}>Back</button>
    </div>
  </form>;
}
