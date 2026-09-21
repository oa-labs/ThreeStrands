import { Check, Clock3, Pencil, Plus, RotateCcw, Sparkles, X } from "lucide-react";
import { forwardRef, useCallback, useEffect, useImperativeHandle, useMemo, useRef, useState } from "react";
import type { ActionProposal, MeetingProposal, ThreadDetail, ThreadTask } from "./domain";
import { mailClient } from "./data/client";
import { useEscapeDismiss } from "./useEscapeDismiss";

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

export const TaskSidebar = forwardRef<TaskWorkspaceHandle, {
  variant?: "sidebar" | "workspace";
  onClose(): void;
  accountId: string | null;
  currentThread: ThreadDetail | null;
  onOpenThread(threadId: string): void;
  onTasksChanged?(): void;
  onCheckSchedule?(): void;
  onAnalyzeThread?(): void;
  analysisEnabled?: boolean;
  analysisReady?: boolean;
  analysisLoading?: boolean;
  analysisError?: string | null;
  analysisPreview?: string | null;
  proposals?: ActionProposal[];
  onDiscardProposal?(index: number): void;
  onReviewProposal?(index: number, proposal: ActionProposal, intent: "edit" | "accept"): void;
  onFindTimesProposal?(proposal: MeetingProposal): void;
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
  onAnalyzeThread,
  analysisEnabled = false,
  analysisReady = false,
  analysisLoading = false,
  analysisError = null,
  analysisPreview = null,
  proposals = [],
  onDiscardProposal,
  onReviewProposal,
  onFindTimesProposal,
  onDraftFollowUp,
  onNewTask,
  onEditTask,
  onSelectedTaskChange,
  refreshKey = 0,
  title = "Tasks",
}, ref) {
  const [tasks, setTasks] = useState<ThreadTask[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [selectedTaskId, setSelectedTaskId] = useState<string | null>(null);
  const taskCards = useRef(new Map<string, HTMLElement>());
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

  useEffect(() => { void load(); }, [load, refreshKey]);
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
  const orderedTasks = useMemo(() => grouped.flatMap((group) => group.tasks), [grouped]);
  const selectedTask = orderedTasks.find((task) => task.id === selectedTaskId) ?? null;

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

  const setStatus = async (task: ThreadTask, status: "open" | "completed") => {
    try {
      const updated = await mailClient.setTaskStatus(task.id, status);
      setTasks((current) => current.map((candidate) => candidate.id === updated.id ? updated : candidate));
      onTasksChanged?.();
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    }
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
    reopenSelected: () => { if (selectedTask?.status === "completed") void setStatus(selectedTask, "open"); },
  }), [moveSelection, onEditTask, onOpenThread, selectedTask]);

  const taskList = <div className="tasks-list">
    {grouped.map((group) => (
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
              {task.subjectSnapshot ? <span>{task.subjectSnapshot}</span> : null}
              {formatDue(task) ? <small><Clock3 size={12} /> {formatDue(task)}</small> : null}
            </button>
            <button type="button" className="task-status-button" aria-label={task.status === "completed" ? `Reopen ${task.title}` : `Complete ${task.title}`} onClick={() => void setStatus(task, task.status === "completed" ? "open" : "completed")}>
              {task.status === "completed" ? <RotateCcw size={15} /> : <Check size={15} />}
            </button>
            {onDraftFollowUp && task.threadId && task.kind === "follow_up" && isDue(task) ? (
              <button type="button" className="task-follow-up-button" onClick={() => onDraftFollowUp(task)}>Draft Follow-Up</button>
            ) : null}
          </article>
        ))}
      </section>
    ))}
  </div>;

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
          {title === "Actions" && onAnalyzeThread ? <button type="button" aria-label="Analyze Thread" title={!analysisEnabled ? "Enable thread actions in AI settings" : !analysisReady ? "Configure an AI provider and API key" : "Analyze thread"} onClick={onAnalyzeThread} disabled={!analysisReady || analysisLoading}><Sparkles size={17} /></button> : null}
          {onCheckSchedule ? <button type="button" aria-label="Check Schedule" title="Check schedule" onClick={onCheckSchedule}><Clock3 size={17} /></button> : null}
          {onNewTask && (variant === "workspace" || currentThread) ? <button type="button" aria-label="Add Task" title="Add task" onClick={onNewTask}><Plus size={17} /></button> : null}
          <button type="button" aria-label="Close Tasks" onClick={onClose}><X size={18} /></button>
        </div>
      </header>
      {error ? <p className="tasks-error" role="alert">{error}</p> : null}
      {title === "Actions" && onAnalyzeThread ? (
        <section className="action-analysis" aria-label="Thread actions">
          <div className="action-analysis-heading"><strong>Thread actions</strong>{analysisLoading ? <span role="status">Analyzing…</span> : null}</div>
          {!analysisEnabled ? <p className="tasks-status">Enable Thread actions in AI settings to analyze this conversation.</p> : !analysisReady ? <p className="tasks-status">Configure an AI provider and API key in AI settings to analyze this conversation.</p> : null}
          {analysisError ? (() => {
            const { summary, retryable } = describeAnalysisError(analysisError);
            return <div className="action-analysis-error" role="alert">
              <p>{summary}</p>
              <div className="action-analysis-error-actions">
                {retryable && onAnalyzeThread ? <button type="button" onClick={onAnalyzeThread}><RotateCcw size={13} /> Try Again</button> : null}
                {retryable ? <details className="action-analysis-error-details"><summary>Technical details</summary><p>{analysisError}</p></details> : null}
              </div>
            </div>;
          })() : null}
          {analysisPreview ? <details className="action-analysis-preview"><summary>Exact bounded content sent</summary><pre>{analysisPreview}</pre></details> : null}
          {!analysisLoading && analysisEnabled && proposals.length === 0 && analysisPreview ? <p className="tasks-status">No meeting or task proposals found.</p> : null}
          <div className="action-proposals">
            {proposals.map((proposal, index) => {
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
                  {onReviewProposal ? <button type="button" onClick={() => onReviewProposal(index, proposal, "edit")}><Pencil size={13} /> Edit</button> : null}
                  {proposal.type === "task" && onReviewProposal ? <button type="button" onClick={() => onReviewProposal(index, proposal, "accept")}>Review &amp; Add Task</button> : null}
                  {proposal.type === "meeting" && onFindTimesProposal ? <button type="button" disabled={needsReview} title={needsReview ? "Edit this proposal before finding times" : undefined} onClick={() => onFindTimesProposal(proposal)}>Find Times</button> : null}
                  {onDiscardProposal ? <button type="button" onClick={() => onDiscardProposal(index)}>Discard</button> : null}
                </div>
              </article>;
            })}
          </div>
        </section>
      ) : null}
      {loading ? <p className="tasks-status">Loading tasks…</p> : null}
      {!loading && grouped.length === 0 ? <p className="tasks-status">No tasks yet. Press d to add one.</p> : null}
      {variant === "workspace" ? <div className="tasks-workspace-body">
        {taskList}
        <section className="task-detail" aria-label="Task details">
          {selectedTask ? <>
            <header>
              <div><span className="eyebrow">{selectedTask.kind.replace("_", " ")}</span><h2>{selectedTask.title}</h2></div>
              <div className="task-detail-actions">
                {onEditTask ? <button type="button" onClick={() => onEditTask(selectedTask)}><Pencil size={14} /> Edit</button> : null}
                <button type="button" onClick={() => void setStatus(selectedTask, selectedTask.status === "completed" ? "open" : "completed")}>
                  {selectedTask.status === "completed" ? <><RotateCcw size={14} /> Reopen</> : <><Check size={14} /> Mark Done</>}
                </button>
                {selectedTask.threadId ? <button type="button" onClick={() => onOpenThread(selectedTask.threadId!)}>Open Conversation</button> : null}
              </div>
            </header>
            <dl>
              <div><dt>Status</dt><dd><span className={`task-status-pill task-status-${selectedTask.status}`}>{selectedTask.status}</span></dd></div>
              <div><dt>Due</dt><dd>{formatDueDetail(selectedTask) ?? "No due date"}</dd></div>
              {selectedTask.subjectSnapshot ? <div><dt>Conversation</dt><dd>{selectedTask.subjectSnapshot}</dd></div> : null}
              {selectedTask.repeatIntervalDays ? <div><dt>Repeats</dt><dd>Every {selectedTask.repeatIntervalDays} days</dd></div> : null}
            </dl>
            <section className="task-detail-notes" aria-label="Notes"><h3>Notes</h3><p>{selectedTask.notes || "No notes"}</p></section>
            {selectedTask.evidenceText && selectedTask.threadId ? (
              <section className="task-detail-notes" aria-label="Evidence">
                <h3>Evidence</h3>
                <button type="button" className="task-detail-evidence" title="Open conversation" onClick={() => onOpenThread(selectedTask.threadId!)}>
                  {selectedTask.evidenceText}
                </button>
              </section>
            ) : null}
            <p className="task-detail-shortcuts"><kbd>j</kbd>/<kbd>k</kbd> move · <kbd>Enter</kbd> edit · {selectedTask.status === "completed" ? <><kbd>Shift+e</kbd> reopen</> : <><kbd>e</kbd> complete</>}{selectedTask.threadId ? <> · <kbd>o</kbd> open conversation</> : null}</p>
          </> : <p className="tasks-status">Select a task to see its details.</p>}
        </section>
      </div> : taskList}
    </section>
  );
});
