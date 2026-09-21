import { Check, CheckSquare, Clock3, Pencil, Plus, RotateCcw, Sparkles, X } from "lucide-react";
import { forwardRef, useCallback, useEffect, useImperativeHandle, useMemo, useRef, useState } from "react";
import type { ActionProposal, MeetingProposal, ThreadDetail, ThreadTask } from "./domain";
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
  toggleSelected(): void;
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
    return ["Today", "Upcoming", "Waiting for", "No due date", "Completed"]
      .map((name) => ({ name, tasks: groups.get(name) ?? [] }))
      .filter((group) => group.tasks.length > 0);
  }, [tasks]);
  const orderedTasks = useMemo(() => grouped.flatMap((group) => group.tasks), [grouped]);

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
      const task = orderedTasks.find((candidate) => candidate.id === selectedTaskId);
      if (task) onOpenThread(task.threadId);
    },
    toggleSelected: () => {
      const task = orderedTasks.find((candidate) => candidate.id === selectedTaskId);
      if (task) void setStatus(task, task.status === "completed" ? "open" : "completed");
    },
  }), [moveSelection, onOpenThread, orderedTasks, selectedTaskId]);

  return (
    <section className={variant === "workspace" ? "tasks-workspace" : "tasks-sidebar"} role={variant === "sidebar" ? "complementary" : "region"} aria-label={title}>
      <header className="tasks-sidebar-header">
        <h2><CheckSquare size={18} /> {title}</h2>
        <div>
          {title === "Actions" && onAnalyzeThread ? <button type="button" aria-label="Analyze thread" title={!analysisEnabled ? "Enable Thread actions in AI settings" : !analysisReady ? "Configure an AI provider and API key" : "Analyze thread"} onClick={onAnalyzeThread} disabled={!analysisReady || analysisLoading}><Sparkles size={17} /></button> : null}
          {onCheckSchedule ? <button type="button" aria-label="Check schedule" title="Check schedule" onClick={onCheckSchedule}><Clock3 size={17} /></button> : null}
          {currentThread && onNewTask ? <button type="button" aria-label="Add task" title="Add task" onClick={onNewTask}><Plus size={17} /></button> : null}
          <button type="button" aria-label="Close tasks" onClick={onClose}><X size={18} /></button>
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
                {retryable && onAnalyzeThread ? <button type="button" onClick={onAnalyzeThread}><RotateCcw size={13} /> Try again</button> : null}
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
                  {proposal.type === "task" && onReviewProposal ? <button type="button" onClick={() => onReviewProposal(index, proposal, "accept")}>Review &amp; add task</button> : null}
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
              <article
                id={`task-${task.id}`}
                ref={(node) => { if (node) taskCards.current.set(task.id, node); else taskCards.current.delete(task.id); }}
                className={`task-card task-${task.status}${variant === "workspace" && selectedTaskId === task.id ? " selected" : ""}`}
                aria-current={variant === "workspace" && selectedTaskId === task.id ? "true" : undefined}
                key={task.id}
              >
                <button type="button" className="task-card-main" onClick={() => { setSelectedTaskId(task.id); onOpenThread(task.threadId); }}>
                  <strong>{task.title}</strong>
                  <span>{task.subjectSnapshot}</span>
                  {formatDue(task) ? <small><Clock3 size={12} /> {formatDue(task)}</small> : null}
                </button>
                <button type="button" className="task-status-button" aria-label={task.status === "completed" ? `Reopen ${task.title}` : `Complete ${task.title}`} onClick={() => void setStatus(task, task.status === "completed" ? "open" : "completed")}>
                  {task.status === "completed" ? <RotateCcw size={15} /> : <Check size={15} />}
                </button>
                {onDraftFollowUp && task.kind === "follow_up" && isDue(task) ? (
                  <button type="button" className="task-follow-up-button" onClick={() => onDraftFollowUp(task)}>Draft follow-up</button>
                ) : null}
              </article>
            ))}
          </section>
        ))}
      </div>
    </section>
  );
});
