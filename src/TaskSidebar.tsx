import { Check, CheckSquare, Clock3, Pencil, Plus, RotateCcw, Sparkles, X } from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";
import type { ActionProposal, MeetingProposal, TaskProposal, ThreadDetail, ThreadTask, TaskDueKind, TaskKind } from "./domain";
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
  onAnalyzeThread,
  analysisEnabled = false,
  analysisLoading = false,
  analysisError = null,
  analysisPreview = null,
  proposals = [],
  onDiscardProposal,
  onUpdateProposal,
  onAddTaskProposal,
  onFindTimesProposal,
  title = "Tasks",
  initialFormOpen = false,
}: {
  onClose(): void;
  accountId: string | null;
  currentThread: ThreadDetail | null;
  onOpenThread(threadId: string): void;
  onTasksChanged?(): void;
  onCheckSchedule?(): void;
  onAnalyzeThread?(): void;
  analysisEnabled?: boolean;
  analysisLoading?: boolean;
  analysisError?: string | null;
  analysisPreview?: string | null;
  proposals?: ActionProposal[];
  onDiscardProposal?(index: number): void;
  onUpdateProposal?(index: number, proposal: ActionProposal): void;
  onAddTaskProposal?(proposal: TaskProposal): Promise<void> | void;
  onFindTimesProposal?(proposal: MeetingProposal): void;
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
  const [editingProposal, setEditingProposal] = useState<number | null>(null);
  const [editingTitle, setEditingTitle] = useState("");
  const [editingDueKind, setEditingDueKind] = useState<TaskDueKind>("none");
  const [editingDueValue, setEditingDueValue] = useState("");
  const [editingEndValue, setEditingEndValue] = useState("");
  const [editingTimeZone, setEditingTimeZone] = useState("");
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

  const beginProposalEdit = (index: number, proposal: ActionProposal) => {
    setEditingProposal(index);
    setEditingTitle(proposal.title);
    setEditingDueKind(proposal.type === "task" ? proposal.dueKind : "none");
    setEditingDueValue(proposal.type === "task" ? proposal.dueValue ?? "" : proposal.normalizedStart ?? proposal.searchRangeStart ?? "");
    setEditingEndValue(proposal.type === "meeting" ? proposal.normalizedEnd ?? proposal.searchRangeEnd ?? "" : "");
    setEditingTimeZone(proposal.timeZone ?? "");
  };

  const saveProposalEdit = (index: number, proposal: ActionProposal) => {
    if (!editingTitle.trim() || !onUpdateProposal) return;
    const updated = proposal.type === "task"
      ? { ...proposal, title: editingTitle.trim(), dueKind: editingDueKind, dueValue: editingDueKind === "none" ? null : editingDueValue || null, timeZone: editingTimeZone || null }
      : { ...proposal, title: editingTitle.trim(), normalizedStart: editingDueValue || null, normalizedEnd: editingEndValue || null, searchRangeStart: null, searchRangeEnd: null, timeZone: editingTimeZone || null };
    onUpdateProposal(index, updated);
    setEditingProposal(null);
    setEditingTitle("");
  };

  return (
    <aside className="tasks-sidebar" aria-label={title}>
      <header className="tasks-sidebar-header">
        <h2><CheckSquare size={18} /> {title}</h2>
        <div>
          {title === "Actions" && onAnalyzeThread ? <button type="button" aria-label="Analyze thread" title={analysisEnabled ? "Analyze thread" : "Enable Thread actions in AI settings"} onClick={onAnalyzeThread} disabled={!analysisEnabled || analysisLoading}><Sparkles size={17} /></button> : null}
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
      {title === "Actions" && onAnalyzeThread ? (
        <section className="action-analysis" aria-label="Thread actions">
          <div className="action-analysis-heading"><strong>Thread actions</strong>{analysisLoading ? <span role="status">Analyzing…</span> : null}</div>
          {!analysisEnabled ? <p className="tasks-status">Enable Thread actions in AI settings to analyze this conversation.</p> : null}
          {analysisError ? <p className="tasks-error" role="alert">{analysisError}</p> : null}
          {analysisPreview ? <details className="action-analysis-preview"><summary>Exact bounded content sent</summary><pre>{analysisPreview}</pre></details> : null}
          {!analysisLoading && analysisEnabled && proposals.length === 0 && analysisPreview ? <p className="tasks-status">No meeting or task proposals found.</p> : null}
          <div className="action-proposals">
            {proposals.map((proposal, index) => {
              const evidence = <details className="proposal-evidence"><summary>Evidence</summary><blockquote>{proposal.evidence.excerpt}</blockquote><small>Message {proposal.evidence.sourceMessageId}</small></details>;
              const editing = editingProposal === index;
              const needsReview = (proposal.type === "meeting" && (!proposal.timeZone || (!proposal.normalizedStart && !proposal.searchRangeStart)))
                || (proposal.type === "task" && proposal.dueKind === "datetime" && !proposal.timeZone);
              const uncertain = proposal.confidence < 0.75;
              return <article className="action-proposal-card" key={`${proposal.type}-${index}`}>
                <div className="action-proposal-card-header"><span className="proposal-kind">{proposal.type === "meeting" ? "Meeting" : "Task"}</span><span>{Math.round(proposal.confidence * 100)}% confidence{uncertain ? " · Uncertain" : ""}{needsReview ? " · Needs review" : ""}</span></div>
                {editing ? <div className="proposal-edit"><input aria-label="Proposal title" value={editingTitle} onChange={(event) => setEditingTitle(event.target.value)} />{proposal.type === "task" ? <><select aria-label="Proposal due type" value={editingDueKind} onChange={(event) => setEditingDueKind(event.target.value as TaskDueKind)}><option value="none">No due date</option><option value="date">Date</option><option value="datetime">Date and time</option></select><input aria-label="Proposal due value" type={editingDueKind === "datetime" ? "datetime-local" : editingDueKind === "date" ? "date" : "text"} value={editingDueValue} onChange={(event) => setEditingDueValue(event.target.value)} disabled={editingDueKind === "none"} /></> : <><input aria-label="Proposal start" placeholder="RFC3339 start" value={editingDueValue} onChange={(event) => setEditingDueValue(event.target.value)} /><input aria-label="Proposal end" placeholder="RFC3339 end" value={editingEndValue} onChange={(event) => setEditingEndValue(event.target.value)} /><input aria-label="Proposal timezone" placeholder="IANA timezone" value={editingTimeZone} onChange={(event) => setEditingTimeZone(event.target.value)} /></>}<button type="button" onClick={() => saveProposalEdit(index, proposal)}>Save</button><button type="button" onClick={() => setEditingProposal(null)}>Cancel</button></div> : <strong>{proposal.title}</strong>}
                {proposal.type === "meeting" ? <><p>{proposal.rawTimeLanguage || "Time not specified"}</p>{proposal.participants.length > 0 ? <p>{proposal.participants.join(", ")}</p> : null}</> : <p>{proposal.notes || proposal.kind.replace("_", " ")}{proposal.dueValue ? ` · Due ${proposal.dueValue}` : ""}</p>}
                {evidence}
                <div className="proposal-actions">
                  <button type="button" onClick={() => beginProposalEdit(index, proposal)}><Pencil size={13} /> Edit</button>
                  {proposal.type === "task" && onAddTaskProposal ? <button type="button" disabled={needsReview} title={needsReview ? "Edit this proposal before adding the task" : undefined} onClick={() => void onAddTaskProposal(proposal)}>Add task</button> : null}
                  {proposal.type === "meeting" && onFindTimesProposal ? <button type="button" disabled={needsReview} title={needsReview ? "Edit this proposal before finding times" : undefined} onClick={() => onFindTimesProposal(proposal)}>Find times</button> : null}
                  {onDiscardProposal ? <button type="button" onClick={() => onDiscardProposal(index)}>Discard</button> : null}
                </div>
              </article>;
            })}
          </div>
        </section>
      ) : null}
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
