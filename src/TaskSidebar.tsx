import { Check, Clock3, MessageSquare, Pencil, Plus, RotateCcw, Sparkles, X } from "lucide-react";
import { forwardRef, useCallback, useEffect, useImperativeHandle, useMemo, useRef, useState } from "react";
import type { ActionProposal, MeetingProposal, ThreadDetail, ThreadTask, UpdateTaskRequest, TaskDueKind } from "./domain";
import { mailClient } from "./data/client";
import { ActionButton, HoverTooltip } from "./AppChrome";
import { useEscapeDismiss } from "./useEscapeDismiss";
import { errorMessage } from "./errors";
import { TASK_VIEWS, taskMatchesView, taskViewForAll, type TaskView } from "./taskViews";

function taskGroup(task: ThreadTask): string {
  if (task.status === "completed") return "Completed";
  if (task.kind === "waiting_for") return "Waiting For";
  if (!task.dueValue) return "No Due Date";
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

function formatDueDetail(task: ThreadTask): string | null {
  if (!task.dueValue) return null;
  const value = task.dueKind === "date" ? new Date(`${task.dueValue}T12:00:00`) : new Date(task.dueValue);
  return task.dueKind === "datetime"
    ? value.toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" })
    : value.toLocaleDateString(undefined, { dateStyle: "medium" });
}

function dateTimeInputValue(value: string | null | undefined): string {
  if (!value) return "";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value.slice(0, 16);
  return new Date(date.getTime() - date.getTimezoneOffset() * 60_000).toISOString().slice(0, 16);
}

function isDue(task: ThreadTask): boolean {
  if (task.status !== "open" || !task.dueValue) return false;
  const due = task.dueKind === "date" ? new Date(`${task.dueValue}T23:59:59`) : new Date(task.dueValue);
  return due.getTime() <= Date.now();
}

function describeAnalysisError(message: string): { summary: string; retryable: boolean } {
  if (/^(the )?ai provider returned/i.test(message) || /^the ai provider (cited|included)/i.test(message)) {
    return {
      summary: "The AI assistant couldn't make sense of this conversation. This sometimes happens with longer or unusual threads.",
      retryable: true,
    };
  }
  if (/error sending request|timed out|connection|dns/i.test(message)) {
    return { summary: "Couldn't reach the AI provider. Check your connection and try again.", retryable: true };
  }
  return { summary: message, retryable: false };
}

export type TaskWorkspaceHandle = {
  selectNext(): void;
  selectPrevious(): void;
  openSelected(): void;
  editSelected(): void;
  completeSelected(): void;
  reopenSelected(): void;
};

/**
 * AI thread-action analysis for the current conversation. Supplying it adds
 * the analyze control and the proposal review list to the panel.
 */
export type ThreadActionAnalysis = {
  enabled: boolean;
  ready: boolean;
  loading: boolean;
  error: string | null;
  preview: string | null;
  proposals: ActionProposal[];
  onAnalyze(): void;
  onDiscardProposal?(index: number): void;
  onReviewProposal?(index: number, proposal: ActionProposal, intent: "edit" | "accept"): void;
  onFindTimesProposal?(proposal: MeetingProposal): void;
};

export const TaskSidebar = forwardRef<TaskWorkspaceHandle, {
  variant?: "sidebar" | "workspace";
  onClose(): void;
  accountId: string | null;
  currentThread: ThreadDetail | null;
  onOpenThread(threadId: string): void;
  onTasksChanged?(): void;
  onCheckSchedule?(): void;
  analysis?: ThreadActionAnalysis;
  onDraftFollowUp?(task: ThreadTask): void;
  onNewTask?(): void;
  onEditTask?(task: ThreadTask): void;
  onSelectedTaskChange?(task: ThreadTask | null): void;
  refreshKey?: number;
  title?: string;
}>(function TaskSidebar({
  variant = "sidebar",
  onClose,
  accountId,
  currentThread,
  onOpenThread,
  onTasksChanged,
  onCheckSchedule,
  analysis,
  onDraftFollowUp,
  onNewTask,
  onEditTask,
  onSelectedTaskChange,
  refreshKey = 0,
  title = "Tasks",
}, ref) {
  const [tasks, setTasks] = useState<ThreadTask[]>([]);
  const [now, setNow] = useState(() => new Date());
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [selectedTaskId, setSelectedTaskId] = useState<string | null>(null);
  const [view, setView] = useState<TaskView>("All");
  const [editing, setEditing] = useState<"title" | "description" | "due" | null>(null);
  const [titleDraft, setTitleDraft] = useState("");
  const [descriptionDraft, setDescriptionDraft] = useState("");
  const [dueKindDraft, setDueKindDraft] = useState<TaskDueKind>("date");
  const [dueValueDraft, setDueValueDraft] = useState("");
  const [saving, setSaving] = useState(false);
  const taskCards = useRef(new Map<string, HTMLElement>());
  useEscapeDismiss(onClose, variant !== "workspace");

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      setTasks(await mailClient.listTasks(accountId ?? undefined));
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setLoading(false);
    }
  }, [accountId]);

  useEffect(() => { void load(); }, [load, refreshKey]);
  useEffect(() => {
    if (variant !== "workspace") return;
    const interval = window.setInterval(() => setNow(new Date()), 60_000);
    return () => window.clearInterval(interval);
  }, [variant]);
  const grouped = useMemo(() => {
    const groups = new Map<string, ThreadTask[]>();
    for (const task of tasks) {
      const group = taskGroup(task);
      const current = groups.get(group) ?? [];
      current.push(task);
      groups.set(group, current);
    }
    return ["Today", "Upcoming", "Waiting For", "No Due Date", "Completed"]
      .map((name) => ({ name, tasks: groups.get(name) ?? [] }))
      .filter((group) => group.tasks.length > 0);
  }, [tasks]);
  const workspaceGroups = useMemo(() => TASK_VIEWS.filter((name) => name !== "All")
    .map((name) => ({ name, tasks: tasks.filter((task) =>
      view === "All" ? taskViewForAll(task, now) === name && task.status === "open" : name === view && taskMatchesView(task, view, now),
    ) }))
    .filter((group) => group.tasks.length > 0), [now, tasks, view]);
  const displayedGroups = variant === "workspace" ? workspaceGroups : grouped;
  const orderedTasks = useMemo(() => displayedGroups.flatMap((group) => group.tasks), [displayedGroups]);
  const selectedTask = orderedTasks.find((task) => task.id === selectedTaskId) ?? null;

  useEffect(() => {
    setEditing(null);
  }, [selectedTaskId]);

  useEffect(() => {
    if (orderedTasks.length === 0) {
      setSelectedTaskId(null);
      return;
    }
    setSelectedTaskId((current) => current && orderedTasks.some((task) => task.id === current) ? current : orderedTasks[0].id);
  }, [orderedTasks]);

  useEffect(() => {
    if (variant === "workspace" && selectedTaskId) taskCards.current.get(selectedTaskId)?.scrollIntoView?.({ block: "nearest" });
  }, [selectedTaskId, variant]);

  useEffect(() => { onSelectedTaskChange?.(selectedTask); }, [onSelectedTaskChange, selectedTask]);

  const setStatus = useCallback(async (task: ThreadTask, status: "open" | "completed") => {
    try {
      const updated = await mailClient.setTaskStatus(task.id, status);
      setTasks((current) => current.map((candidate) => candidate.id === updated.id ? updated : candidate));
      onTasksChanged?.();
    } catch (reason) {
      setError(errorMessage(reason));
    }
  }, [onTasksChanged]);

  const saveTask = async (request: Omit<UpdateTaskRequest, "id">) => {
    if (!selectedTask || saving) return;
    setSaving(true);
    setError(null);
    try {
      const updated = await mailClient.updateTask({ id: selectedTask.id, ...request });
      setTasks((current) => current.map((task) => task.id === updated.id ? updated : task));
      setEditing(null);
      onTasksChanged?.();
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setSaving(false);
    }
  };

  const startEditing = (field: "title" | "description" | "due") => {
    if (!selectedTask) return;
    setError(null);
    setTitleDraft(selectedTask.title);
    setDescriptionDraft(selectedTask.notes ?? "");
    setDueKindDraft(selectedTask.dueKind === "none" ? "date" : selectedTask.dueKind);
    setDueValueDraft(selectedTask.dueKind === "datetime" ? dateTimeInputValue(selectedTask.dueValue) : selectedTask.dueValue ?? "");
    setEditing(field);
  };

  const moveSelection = useCallback((direction: -1 | 1) => {
    if (orderedTasks.length === 0) return;
    const currentIndex = orderedTasks.findIndex((task) => task.id === selectedTaskId);
    const from = currentIndex === -1 ? 0 : currentIndex;
    setSelectedTaskId(orderedTasks[(from + direction + orderedTasks.length) % orderedTasks.length].id);
  }, [orderedTasks, selectedTaskId]);

  useImperativeHandle(ref, () => ({
    selectNext: () => moveSelection(1),
    selectPrevious: () => moveSelection(-1),
    openSelected: () => {
      if (selectedTask?.threadId) onOpenThread(selectedTask.threadId);
    },
    editSelected: () => { if (selectedTask) onEditTask?.(selectedTask); },
    completeSelected: () => { if (selectedTask?.status === "open") void setStatus(selectedTask, "completed"); },
    reopenSelected: () => { if (selectedTask && selectedTask.status !== "open") void setStatus(selectedTask, "open"); },
  }), [moveSelection, onEditTask, onOpenThread, selectedTask, setStatus]);

  const taskList = <div className="tasks-list">
    {displayedGroups.map((group) => (
      <section key={group.name} aria-labelledby={`task-group-${group.name.replace(/\s/g, "-")}`}>
        <h3 id={`task-group-${group.name.replace(/\s/g, "-")}`}>{group.name}</h3>
        {group.tasks.map((task) => (
          <article
            id={`task-${task.id}`}
            ref={(node) => { if (node) taskCards.current.set(task.id, node); else taskCards.current.delete(task.id); }}
            className={`task-card task-${task.status}${variant === "workspace" && selectedTaskId === task.id ? " selected" : ""}`}
            aria-current={variant === "workspace" && selectedTaskId === task.id ? "true" : undefined}
            key={task.id}
          >
            <button type="button" className="task-card-main" onClick={() => variant === "workspace" || !task.threadId ? setSelectedTaskId(task.id) : onOpenThread(task.threadId)}>
              <strong>{task.title}</strong>
              {variant === "sidebar" && task.subjectSnapshot ? <span>{task.subjectSnapshot}</span> : null}
              {formatDue(task) ? <small><Clock3 size={12} /> {formatDue(task)}</small> : null}
            </button>
            <button type="button" className="task-status-button" aria-label={task.status !== "open" ? `Reopen ${task.title}` : `Complete ${task.title}`} onClick={() => void setStatus(task, task.status !== "open" ? "open" : "completed")}>
              {task.status !== "open" ? <RotateCcw size={15} /> : <Check size={15} />}
            </button>
            {onDraftFollowUp && task.threadId && task.kind === "follow_up" && isDue(task) ? (
              <button type="button" className="task-follow-up-button" onClick={() => onDraftFollowUp(task)}>Draft Follow-Up</button>
            ) : null}
          </article>
        ))}
      </section>
    ))}
  </div>;

  const analysisActionLabel = !analysis?.enabled
    ? "Enable thread actions in AI settings"
    : !analysis.ready
      ? "Configure an AI provider and API key"
      : "Analyze thread";

  return (
    <section className={variant === "workspace" ? "tasks-workspace" : "tasks-sidebar"} role={variant === "sidebar" ? "complementary" : "region"} aria-label={title}>
      <header className="tasks-sidebar-header">
        <div className="thread-header-title">
          <div>
            <span className="eyebrow">
              {title}
              {accountId ? <span className="eyebrow-account"> · {accountId}</span> : null}
            </span>
            <h1>{orderedTasks.length} {orderedTasks.length === 1 ? "task" : "tasks"}</h1>
          </div>
        </div>
        <div className="tasks-sidebar-header-actions">
          {analysis ? <HoverTooltip title={analysisActionLabel}><button type="button" aria-label={analysisActionLabel} onClick={analysis.onAnalyze} disabled={!analysis.ready || analysis.loading}><Sparkles size={17} /></button></HoverTooltip> : null}
          {variant === "sidebar" && onCheckSchedule ? <HoverTooltip title="Check schedule"><button type="button" aria-label="Check schedule" onClick={onCheckSchedule}><Clock3 size={17} /></button></HoverTooltip> : null}
          {onNewTask && (variant === "workspace" || currentThread) ? <HoverTooltip title="Add task"><button type="button" className={variant === "workspace" ? "task-add-button" : undefined} aria-label="Add task" onClick={onNewTask}><Plus size={17} />{variant === "workspace" ? "Add task" : null}</button></HoverTooltip> : null}
          {variant === "workspace" ? null : <button type="button" aria-label="Close Tasks" onClick={onClose}><X size={18} /></button>}
        </div>
      </header>
      {error ? <p className="form-error tasks-error" role="alert">{error}</p> : null}
      {analysis ? (
        <section className="action-analysis" aria-label="Thread actions">
          <div className="action-analysis-heading"><strong>Thread actions</strong>{analysis.loading ? <span role="status">Analyzing…</span> : null}</div>
          {!analysis.enabled ? <p className="tasks-status">Enable Thread actions in AI settings to analyze this conversation.</p> : !analysis.ready ? <p className="tasks-status">Configure an AI provider and API key in AI settings to analyze this conversation.</p> : null}
          {analysis.error ? (() => {
            const { summary, retryable } = describeAnalysisError(analysis.error);
            return <div className="action-analysis-error" role="alert">
              <p>{summary}</p>
              <div className="action-analysis-error-actions">
                {retryable ? <button type="button" onClick={analysis.onAnalyze}><RotateCcw size={13} /> Try Again</button> : null}
                {retryable ? <details className="action-analysis-error-details"><summary>Technical details</summary><p>{analysis.error}</p></details> : null}
              </div>
            </div>;
          })() : null}
          {analysis.preview ? <details className="action-analysis-preview"><summary>Exact bounded content sent</summary><pre>{analysis.preview}</pre></details> : null}
          {!analysis.loading && analysis.enabled && analysis.proposals.length === 0 && analysis.preview ? <p className="tasks-status">No meeting or task proposals found.</p> : null}
          <div className="action-proposals">
            {analysis.proposals.map((proposal, index) => {
              const evidence = <details className="proposal-evidence"><summary>Evidence</summary><blockquote>{proposal.evidence.excerpt}</blockquote><small>Message {proposal.evidence.sourceMessageId}</small></details>;
              const needsReview = (proposal.type === "meeting" && (!proposal.timeZone || (!proposal.normalizedStart && !proposal.searchRangeStart)))
                || (proposal.type === "task" && proposal.dueKind === "datetime" && !proposal.timeZone);
              const uncertain = proposal.confidence < 0.75;
              return <article className="action-proposal-card" key={`${proposal.type}-${index}`}>
                <div className="action-proposal-card-header"><span className="proposal-kind">{proposal.type === "meeting" ? "Meeting" : "Task"}</span><span>{Math.round(proposal.confidence * 100)}% confidence{uncertain ? " · Uncertain" : ""}{needsReview ? " · Needs review" : ""}</span></div>
                <strong>{proposal.title}</strong>
                {proposal.type === "meeting" ? <><p>{proposal.rawTimeLanguage || "Time not specified"}</p>{proposal.location ? <p>{proposal.location}</p> : null}{proposal.participants.length > 0 ? <p>{proposal.participants.join(", ")}</p> : null}</> : <p>{proposal.notes || proposal.kind.replace("_", " ")}{proposal.dueValue ? ` · Due ${proposal.dueValue}` : ""}</p>}
                {evidence}
                <div className="proposal-actions">
                  {analysis.onReviewProposal ? <button type="button" onClick={() => analysis.onReviewProposal?.(index, proposal, "edit")}><Pencil size={13} /> Edit</button> : null}
                  {proposal.type === "task" && analysis.onReviewProposal ? <button type="button" onClick={() => analysis.onReviewProposal?.(index, proposal, "accept")}>Review &amp; Add Task</button> : null}
                  {proposal.type === "meeting" && analysis.onFindTimesProposal ? <button type="button" disabled={needsReview} title={needsReview ? "Edit this proposal before finding times" : undefined} onClick={() => analysis.onFindTimesProposal?.(proposal)}>Find Times</button> : null}
                  {analysis.onDiscardProposal ? <button type="button" onClick={() => analysis.onDiscardProposal?.(index)}>Discard</button> : null}
                </div>
              </article>;
            })}
          </div>
        </section>
      ) : null}
      {loading ? <p className="tasks-status">Loading tasks…</p> : null}
      {!loading && tasks.length === 0 ? <p className="tasks-status">No tasks yet. Press d to add one.</p> : null}
      {variant === "workspace" ? <div className="tasks-workspace-body">
        <div className="tasks-list-pane">
          <nav className="task-view-nav" aria-label="Task views">
            {TASK_VIEWS.map((name) => <button key={name} type="button" aria-pressed={view === name} onClick={() => setView(name)}>
              <span>{name}</span><span className="task-view-count" aria-hidden="true">{tasks.filter((task) => taskMatchesView(task, name, now)).length}</span>
            </button>)}
          </nav>
          {!loading && tasks.length > 0 && displayedGroups.length === 0 ? <p className="tasks-status">{view === "All" ? "No open tasks. Add a task or view completed work." : `No tasks in ${view.toLowerCase()}.`}</p> : null}
          {taskList}
        </div>
        <section className="task-detail" aria-label="Task details">
          {selectedTask ? <>
            <header>
              <div className="task-detail-heading">
                <span className="eyebrow">{selectedTask.kind.replace("_", " ")}</span>
                <div className="task-detail-title-row">
                  <button type="button" className="task-detail-complete" aria-label={selectedTask.status !== "open" ? `Reopen ${selectedTask.title}` : `Complete ${selectedTask.title}`} onClick={() => void setStatus(selectedTask, selectedTask.status !== "open" ? "open" : "completed")}>
                    {selectedTask.status !== "open" ? <RotateCcw size={20} /> : <Check size={20} />}
                  </button>
                  {editing === "title" ? <form className="task-inline-title" data-shortcut-scope="modal" onSubmit={(event) => { event.preventDefault(); if (titleDraft.trim()) void saveTask({ title: titleDraft.trim() }); }} onKeyDown={(event) => { if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); setEditing(null); } }}>
                    <input autoFocus aria-label="Task title" value={titleDraft} onChange={(event) => setTitleDraft(event.target.value)} maxLength={240} />
                    <button type="submit" disabled={saving || !titleDraft.trim()}>Save</button><button type="button" onClick={() => setEditing(null)}>Cancel</button>
                  </form> : <h2><button type="button" className="task-detail-title-button" aria-label={`Edit title: ${selectedTask.title}`} onClick={() => startEditing("title")}>{selectedTask.title}</button></h2>}
                </div>
              </div>
              <div className="task-detail-actions">
                {onEditTask ? <HoverTooltip label="Edit" shortcut="Enter" placement="bottom">
                  <ActionButton label="Edit" shortcut="Enter" onClick={() => onEditTask(selectedTask)}><Pencil size={17} /></ActionButton>
                </HoverTooltip> : null}
              </div>
            </header>
            <section className="task-detail-description" aria-label="Description">
              <h3>Description</h3>
              {editing === "description" ? <form data-shortcut-scope="modal" onSubmit={(event) => { event.preventDefault(); void saveTask({ notes: descriptionDraft.trim() || null }); }} onKeyDown={(event) => { if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); setEditing(null); } }}>
                <textarea autoFocus aria-label="Description" value={descriptionDraft} onChange={(event) => setDescriptionDraft(event.target.value)} rows={5} maxLength={8000} />
                <div className="task-inline-actions"><button type="submit" disabled={saving}>Save</button><button type="button" onClick={() => setEditing(null)}>Cancel</button></div>
              </form> : <button type="button" className="task-detail-description-button" onClick={() => startEditing("description")}>{selectedTask.notes || "Add a description"}</button>}
            </section>
            <section className="task-detail-schedule" aria-label="Schedule">
              <h3>Due date</h3>
              {editing === "due" ? <form data-shortcut-scope="modal" onSubmit={(event) => { event.preventDefault(); if (!dueValueDraft) return; void saveTask({ dueKind: dueKindDraft, dueValue: dueKindDraft === "datetime" ? new Date(dueValueDraft).toISOString() : dueValueDraft, timeZone: dueKindDraft === "datetime" ? Intl.DateTimeFormat().resolvedOptions().timeZone : null }); }} onKeyDown={(event) => { if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); setEditing(null); } }}>
                <select aria-label="Due type" value={dueKindDraft} onChange={(event) => { setDueKindDraft(event.target.value as TaskDueKind); setDueValueDraft(""); }}><option value="date">Date</option><option value="datetime">Date and time</option></select>
                <input aria-label={dueKindDraft === "datetime" ? "Due date and time" : "Due date"} type={dueKindDraft === "datetime" ? "datetime-local" : "date"} value={dueValueDraft} onChange={(event) => setDueValueDraft(event.target.value)} required />
                <div className="task-inline-actions"><button type="submit" disabled={saving || !dueValueDraft}>Save</button>{selectedTask.dueKind !== "none" ? <button type="button" disabled={saving} onClick={() => void saveTask({ dueKind: "none", dueValue: null, timeZone: null })}>Clear date</button> : null}<button type="button" onClick={() => setEditing(null)}>Cancel</button></div>
              </form> : <button type="button" className="task-detail-due-button" onClick={() => startEditing("due")}>{formatDueDetail(selectedTask) ?? "Add a due date"}</button>}
              {selectedTask.repeatIntervalDays ? <p>Repeats every {selectedTask.repeatIntervalDays} days</p> : null}
            </section>
            {selectedTask.threadId ? <section className="task-detail-source" aria-label="Source conversation">
              <h3>Source conversation</h3>
              {selectedTask.subjectSnapshot ? <p>{selectedTask.subjectSnapshot}</p> : null}
              <button type="button" onClick={() => onOpenThread(selectedTask.threadId!)}><MessageSquare size={16} /> Open conversation</button>
              {selectedTask.evidenceText ? <details><summary>Source excerpt</summary><blockquote>{selectedTask.evidenceText}</blockquote></details> : null}
            </section> : null}
            <p className="task-detail-shortcuts"><kbd>j</kbd>/<kbd>k</kbd> move · <kbd>Enter</kbd> edit · {selectedTask.status !== "open" ? <><kbd>Shift+e</kbd> reopen</> : <><kbd>e</kbd> complete</>}{selectedTask.threadId ? <> · <kbd>o</kbd> open conversation</> : null}</p>
          </> : <p className="tasks-status">Select a task to see its details.</p>}
        </section>
      </div> : taskList}
    </section>
  );
});
