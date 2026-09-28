import { Check, ChevronLeft, ChevronRight, Clock3, Columns3, List, MessageSquare, Pencil, Plus, RotateCcw, Sparkles, X } from "lucide-react";
import { forwardRef, useCallback, useEffect, useImperativeHandle, useMemo, useRef, useState, type CSSProperties } from "react";
import type { ActionProposal, MeetingProposal, ThreadDetail, ThreadTask, UpdateTaskRequest, TaskDueKind, TaskStatus } from "./domain";
import { mailClient } from "./data/client";
import { ActionButton, HoverTooltip } from "./AppChrome";
import { convertDueInputValue, isValidTimeZone, listSupportedTimeZones } from "./calendarTime";
import { useEscapeDismiss } from "./useEscapeDismiss";
import { PanelResizeHandle, useTaskDetailWidth } from "./PanelResizeHandle";
import { errorMessage } from "./errors";
import { adjacentTaskStatus, dueView, isActiveTaskStatus, TASK_BOARD_COLUMNS, TASK_VIEWS, taskBoardColumn, taskMatchesView, taskViewForAll, type TaskView } from "./taskViews";

export type TaskLayout = "board" | "list";
const TASK_LAYOUT_KEY = "threestrands.tasks.layout";

function readTaskLayout(): TaskLayout {
  try {
    return localStorage.getItem(TASK_LAYOUT_KEY) === "list" ? "list" : "board";
  } catch {
    return "board";
  }
}

const STATUS_LABELS: Record<TaskStatus, string> = { open: "To Do", in_progress: "In Progress", completed: "Done", cancelled: "Done" };

function taskGroup(task: ThreadTask): string {
  if (task.status === "completed") return "Completed";
  if (task.kind === "waiting_for") return "Waiting For";
  if (!task.dueValue) return "No Due Date";
  return dueView(task, new Date()) ?? "No Due Date";
}

function isOverdue(task: ThreadTask): boolean {
  return isActiveTaskStatus(task.status) && dueView(task, new Date()) === "Overdue";
}

function formatRelativeDate(date: Date, now: Date = new Date()): string {
  const startOfDay = (value: Date) => new Date(value.getFullYear(), value.getMonth(), value.getDate()).getTime();
  const diffDays = Math.round((startOfDay(date) - startOfDay(now)) / 86_400_000);
  if (diffDays === 0) return "Today";
  if (diffDays === 1) return "Tomorrow";
  if (diffDays === -1) return "Yesterday";
  const includeYear = date.getFullYear() !== now.getFullYear();
  return date.toLocaleDateString(undefined, includeYear ? { year: "numeric", month: "short", day: "numeric" } : { month: "short", day: "numeric" });
}

function formatDue(task: ThreadTask): string | null {
  if (!task.dueValue) return null;
  const value = task.dueKind === "date" ? new Date(`${task.dueValue}T12:00:00`) : new Date(task.dueValue);
  return formatRelativeDate(value);
}

function formatDueDetail(task: ThreadTask): string | null {
  if (!task.dueValue) return null;
  const value = task.dueKind === "date" ? new Date(`${task.dueValue}T12:00:00`) : new Date(task.dueValue);
  const relative = formatRelativeDate(value);
  return task.dueKind === "datetime" ? `${relative}, ${value.toLocaleTimeString(undefined, { timeStyle: "short" })}` : relative;
}

function dateTimeInputValue(value: string | null | undefined): string {
  if (!value) return "";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value.slice(0, 16);
  return new Date(date.getTime() - date.getTimezoneOffset() * 60_000).toISOString().slice(0, 16);
}

const COMPLETED_RETENTION_MS = 3 * 24 * 60 * 60 * 1000;

function isStaleCompleted(task: ThreadTask): boolean {
  if (task.status !== "completed" || !task.completedAt) return false;
  return Date.now() - new Date(task.completedAt).getTime() > COMPLETED_RETENTION_MS;
}

function isDue(task: ThreadTask): boolean {
  if (!isActiveTaskStatus(task.status) || !task.dueValue) return false;
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
  startNew(): void;
  selectNext(): void;
  selectPrevious(): void;
  openSelected(): void;
  editSelected(): void;
  completeSelected(): void;
  reopenSelected(): void;
  moveSelected(direction: -1 | 1): void;
  selectAdjacentColumn(direction: -1 | 1): void;
  toggleLayout(): void;
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
  accountOptions?: string[];
  currentThread: ThreadDetail | null;
  onOpenThread(threadId: string): void;
  onTasksChanged?(): void;
  onCheckSchedule?(): void;
  analysis?: ThreadActionAnalysis;
  onDraftFollowUp?(task: ThreadTask): void;
  onNewTask?(): void;
  onCreateTask?(title: string, accountId?: string): Promise<ThreadTask>;
  onEditTask?(task: ThreadTask): void;
  onSelectedTaskChange?(task: ThreadTask | null): void;
  onLayoutChange?(layout: TaskLayout): void;
  refreshKey?: number;
  title?: string;
}>(function TaskSidebar({
  variant = "sidebar",
  onClose,
  accountId,
  accountOptions = [],
  currentThread,
  onOpenThread,
  onTasksChanged,
  onCheckSchedule,
  analysis,
  onDraftFollowUp,
  onNewTask,
  onCreateTask,
  onEditTask,
  onSelectedTaskChange,
  onLayoutChange,
  refreshKey = 0,
  title = "Tasks",
}, ref) {
  const [tasks, setTasks] = useState<ThreadTask[]>([]);
  const [now, setNow] = useState(() => new Date());
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [selectedTaskId, setSelectedTaskId] = useState<string | null>(null);
  const [view, setView] = useState<TaskView>("All");
  const [layout, setLayout] = useState<TaskLayout>(readTaskLayout);
  const board = variant === "workspace" && layout === "board";
  const detailSize = useTaskDetailWidth();
  const [editing, setEditing] = useState<"title" | "description" | "due" | null>(null);
  const [titleDraft, setTitleDraft] = useState("");
  const [descriptionDraft, setDescriptionDraft] = useState("");
  const [dueKindDraft, setDueKindDraft] = useState<TaskDueKind>("date");
  const [dueValueDraft, setDueValueDraft] = useState("");
  const [timeZoneDraft, setTimeZoneDraft] = useState("");
  const [timeZoneError, setTimeZoneError] = useState<string | null>(null);
  const timeZones = useMemo(() => listSupportedTimeZones(), []);
  const [saving, setSaving] = useState(false);
  const [addingTask, setAddingTask] = useState(false);
  const [newTaskTitle, setNewTaskTitle] = useState("");
  const [newTaskAccountId, setNewTaskAccountId] = useState("");
  const [creatingTask, setCreatingTask] = useState(false);
  const [announcement, setAnnouncement] = useState("");
  const [completionToast, setCompletionToast] = useState<ThreadTask | null>(null);
  const completionToastTimeout = useRef<number | null>(null);
  const taskCards = useRef(new Map<string, HTMLElement>());
  const newTaskInput = useRef<HTMLInputElement>(null);
  useEscapeDismiss(onClose, variant !== "workspace");

  const clearCompletionToast = useCallback(() => {
    if (completionToastTimeout.current === null) return;
    window.clearTimeout(completionToastTimeout.current);
    completionToastTimeout.current = null;
  }, []);
  useEffect(() => clearCompletionToast, [clearCompletionToast]);

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
      if (isStaleCompleted(task)) continue;
      const group = taskGroup(task);
      const current = groups.get(group) ?? [];
      current.push(task);
      groups.set(group, current);
    }
    return ["Overdue", "Today", "Upcoming", "Waiting For", "No Due Date", "Completed"]
      .map((name) => ({ name, tasks: groups.get(name) ?? [] }))
      .filter((group) => group.tasks.length > 0);
  }, [tasks]);
  const workspaceGroups = useMemo(() => TASK_VIEWS.filter((name) => name !== "All")
    .map((name) => ({ name, tasks: tasks.filter((task) =>
      view === "All" ? taskViewForAll(task, now) === name && isActiveTaskStatus(task.status) : name === view && taskMatchesView(task, view, now),
    ) }))
    .filter((group) => group.tasks.length > 0), [now, tasks, view]);
  const boardColumns = useMemo(() => TASK_BOARD_COLUMNS.map((name) => ({ name, tasks: tasks.filter((task) =>
    taskBoardColumn(task.status) === name
    && (view === "All" ? !isStaleCompleted(task) : taskMatchesView(task, view, now)),
  ) })), [now, tasks, view]);
  const displayedGroups = board ? boardColumns : variant === "workspace" ? workspaceGroups : grouped;
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
  useEffect(() => { if (variant === "workspace") onLayoutChange?.(layout); }, [layout, onLayoutChange, variant]);

  const changeLayout = useCallback((next: TaskLayout) => {
    setLayout(next);
    try {
      localStorage.setItem(TASK_LAYOUT_KEY, next);
    } catch {
      // The chosen layout still applies for this session.
    }
  }, []);

  const setStatus = useCallback(async (task: ThreadTask, status: TaskStatus) => {
    try {
      const previous = tasks.find((candidate) => candidate.id === task.id)?.status ?? task.status;
      const updated = await mailClient.setTaskStatus(task.id, status);
      setTasks((current) => current.map((candidate) => candidate.id === updated.id ? updated : candidate));
      const verb = status === "completed" ? "Completed"
        : status === "in_progress" ? "Started"
          : isActiveTaskStatus(previous) ? "Moved to To Do" : "Reopened";
      setAnnouncement(`${verb}: ${updated.title}`);
      clearCompletionToast();
      if (status === "completed") {
        setCompletionToast(task);
        completionToastTimeout.current = window.setTimeout(() => {
          completionToastTimeout.current = null;
          setCompletionToast(null);
        }, 6000);
      } else {
        setCompletionToast(null);
      }
      onTasksChanged?.();
    } catch (reason) {
      setError(errorMessage(reason));
    }
  }, [clearCompletionToast, onTasksChanged, tasks]);

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

  const startEditing = useCallback((field: "title" | "description" | "due") => {
    if (!selectedTask) return;
    setError(null);
    setTitleDraft(selectedTask.title);
    setDescriptionDraft(selectedTask.notes ?? "");
    setDueKindDraft(selectedTask.dueKind === "none" ? "date" : selectedTask.dueKind);
    setDueValueDraft(selectedTask.dueKind === "datetime" ? dateTimeInputValue(selectedTask.dueValue) : selectedTask.dueValue ?? "");
    setTimeZoneDraft(selectedTask.timeZone ?? Intl.DateTimeFormat().resolvedOptions().timeZone ?? "UTC");
    setTimeZoneError(null);
    setEditing(field);
  }, [selectedTask]);

  const startNew = useCallback(() => {
    setView("All");
    setError(null);
    setNewTaskTitle("");
    setNewTaskAccountId("");
    setAddingTask(true);
    newTaskInput.current?.focus();
  }, []);

  const createTask = async () => {
    const title = newTaskTitle.trim();
    if (!onCreateTask || !title || creatingTask || (!accountId && accountOptions.length > 1 && !newTaskAccountId)) return;
    setCreatingTask(true);
    setError(null);
    try {
      const created = accountId ? await onCreateTask(title) : await onCreateTask(title, newTaskAccountId || accountOptions[0]);
      setTasks((current) => [created, ...current]);
      setView("All");
      setSelectedTaskId(created.id);
      setNewTaskTitle("");
      onTasksChanged?.();
      newTaskInput.current?.focus();
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setCreatingTask(false);
    }
  };

  const moveSelection = useCallback((direction: -1 | 1) => {
    if (orderedTasks.length === 0) return;
    const currentIndex = orderedTasks.findIndex((task) => task.id === selectedTaskId);
    const from = currentIndex === -1 ? 0 : currentIndex;
    setSelectedTaskId(orderedTasks[(from + direction + orderedTasks.length) % orderedTasks.length].id);
  }, [orderedTasks, selectedTaskId]);

  useImperativeHandle(ref, () => ({
    startNew,
    selectNext: () => moveSelection(1),
    selectPrevious: () => moveSelection(-1),
    openSelected: () => {
      if (selectedTask?.threadId) onOpenThread(selectedTask.threadId);
    },
    editSelected: () => { if (selectedTask) startEditing("title"); },
    completeSelected: () => { if (selectedTask && isActiveTaskStatus(selectedTask.status)) void setStatus(selectedTask, "completed"); },
    reopenSelected: () => { if (selectedTask && !isActiveTaskStatus(selectedTask.status)) void setStatus(selectedTask, "open"); },
    moveSelected: (direction) => {
      const next = selectedTask ? adjacentTaskStatus(selectedTask.status, direction) : null;
      if (selectedTask && next) void setStatus(selectedTask, next);
    },
    selectAdjacentColumn: (direction) => {
      if (!board) return;
      const from = selectedTask ? TASK_BOARD_COLUMNS.indexOf(taskBoardColumn(selectedTask.status)) : 0;
      const row = selectedTask ? boardColumns[from].tasks.indexOf(selectedTask) : 0;
      for (let index = from + direction; index >= 0 && index < boardColumns.length; index += direction) {
        const target = boardColumns[index].tasks;
        if (target.length > 0) {
          setSelectedTaskId(target[Math.min(row, target.length - 1)].id);
          return;
        }
      }
    },
    toggleLayout: () => { if (variant === "workspace") changeLayout(layout === "board" ? "list" : "board"); },
  }), [board, boardColumns, changeLayout, layout, moveSelection, onOpenThread, selectedTask, setStatus, startEditing, startNew, variant]);

  const renderCard = (task: ThreadTask) => {
    const active = isActiveTaskStatus(task.status);
    const back = board ? adjacentTaskStatus(task.status, -1) : null;
    const forward = board ? adjacentTaskStatus(task.status, 1) : null;
    return <article
      id={`task-${task.id}`}
      ref={(node) => { if (node) taskCards.current.set(task.id, node); else taskCards.current.delete(task.id); }}
      className={`task-card task-${task.status}${selectedTaskId === task.id ? " selected" : ""}`}
      aria-current={selectedTaskId === task.id ? "true" : undefined}
      key={task.id}
    >
      <button type="button" className="task-card-main" onClick={() => variant === "workspace" || !task.threadId ? setSelectedTaskId(task.id) : onOpenThread(task.threadId)}>
        <strong>{task.title}</strong>
        {variant === "sidebar" && task.subjectSnapshot ? <span>{task.subjectSnapshot}</span> : null}
        {!board && task.status === "in_progress" ? <span className="task-progress-badge">In progress</span> : null}
        {formatDue(task) ? <small className={isOverdue(task) ? "task-due-overdue" : undefined}><Clock3 size={12} /> {formatDue(task)}</small> : null}
      </button>
      {board ? <div className="task-board-moves">
        {back ? <button type="button" className="task-status-button" aria-label={`Move ${task.title} to ${STATUS_LABELS[back]}`} onClick={() => void setStatus(task, back)}><ChevronLeft size={15} /></button> : null}
        {forward ? <button type="button" className="task-status-button" aria-label={`Move ${task.title} to ${STATUS_LABELS[forward]}`} onClick={() => void setStatus(task, forward)}><ChevronRight size={15} /></button> : null}
      </div> : <button type="button" className="task-status-button" aria-label={active ? `Complete ${task.title}` : `Reopen ${task.title}`} onClick={() => void setStatus(task, active ? "completed" : "open")}>
        {active ? <Check size={15} /> : <RotateCcw size={15} />}
      </button>}
      {onDraftFollowUp && task.threadId && task.kind === "follow_up" && isDue(task) ? (
        <button type="button" className="task-follow-up-button" onClick={() => onDraftFollowUp(task)}>Draft Follow-Up</button>
      ) : null}
    </article>;
  };

  const taskList = board ? <div className="task-board">
    {boardColumns.map((column) => {
      const headingId = `task-column-${column.name.replace(/\s/g, "-")}`;
      return <section key={column.name} className="task-board-column" aria-labelledby={headingId}>
        <header><h3 id={headingId}>{column.name}</h3><span className="task-view-count">{column.tasks.length}</span></header>
        <div className="task-board-cards">
          {column.tasks.length > 0 ? column.tasks.map(renderCard) : <p className="task-board-empty">No tasks</p>}
        </div>
      </section>;
    })}
  </div> : <div className="tasks-list">
    {displayedGroups.map((group) => (
      <section key={group.name} aria-labelledby={`task-group-${group.name.replace(/\s/g, "-")}`}>
        <h3 id={`task-group-${group.name.replace(/\s/g, "-")}`}>{group.name}</h3>
        {group.tasks.map(renderCard)}
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
              <span className="eyebrow-account"> · {accountId ?? "All accounts"}</span>
            </span>
            <h1>{orderedTasks.length} {orderedTasks.length === 1 ? "task" : "tasks"}</h1>
          </div>
        </div>
        <div className="tasks-sidebar-header-actions">
          {analysis ? <HoverTooltip title={analysisActionLabel}><button type="button" aria-label={analysisActionLabel} onClick={analysis.onAnalyze} disabled={!analysis.ready || analysis.loading}><Sparkles size={17} /></button></HoverTooltip> : null}
          {variant === "sidebar" && onCheckSchedule ? <HoverTooltip title="Check schedule"><button type="button" aria-label="Check schedule" onClick={onCheckSchedule}><Clock3 size={17} /></button></HoverTooltip> : null}
          {variant === "workspace" ? <div className="task-layout-toggle" role="group" aria-label="Task layout">
            <button type="button" aria-pressed={layout === "list"} onClick={() => changeLayout("list")}><List size={15} />List</button>
            <button type="button" aria-pressed={layout === "board"} onClick={() => changeLayout("board")}><Columns3 size={15} />Board</button>
          </div> : null}
          {variant === "workspace" && onCreateTask ? <button type="button" className="task-add-button" onClick={startNew}><Plus size={17} />Add task</button> : null}
          {variant === "sidebar" && onNewTask && currentThread ? <HoverTooltip title="Add task"><button type="button" aria-label="Add task" onClick={onNewTask}><Plus size={17} /></button></HoverTooltip> : null}
          {variant === "workspace" ? null : <button type="button" aria-label="Close Tasks" onClick={onClose}><X size={18} /></button>}
        </div>
      </header>
      {error ? <p className="form-error tasks-error" role="alert">
        <span>{error}</span>
        <button type="button" aria-label="Dismiss error" onClick={() => setError(null)}><X size={13} /></button>
      </p> : null}
      <p className="sr-only" role="status" aria-live="polite">{announcement}</p>
      {completionToast ? (
        <div className="toast task-complete-toast" role="status">
          Completed &ldquo;{completionToast.title}&rdquo;
          <button type="button" onClick={() => void setStatus(completionToast, isActiveTaskStatus(completionToast.status) ? completionToast.status : "open")}>Undo</button>
        </div>
      ) : null}
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
      {!loading && tasks.length === 0 && !addingTask ? <p className="tasks-status">No tasks yet. Press d to add one.</p> : null}
      {variant === "workspace" ? <div className={`tasks-workspace-body${board ? " tasks-board-layout" : ""}`} style={board ? { "--task-detail-width": `${detailSize.width}px` } as CSSProperties : undefined}>
        <div className={board ? "tasks-board-pane" : "tasks-list-pane"}>
          {board ? <PanelResizeHandle {...detailSize} panelSide="right" label="Resize task detail" controlsId="task-detail-panel" title="Drag to resize the task detail. Use arrow keys to adjust; double-click to reset." /> : null}
          <nav className="task-view-nav" aria-label="Task views">
            {TASK_VIEWS.map((name) => <button key={name} type="button" aria-pressed={view === name} onClick={() => setView(name)}>
              <span>{name}</span><span className="task-view-count" aria-hidden="true">{tasks.filter((task) => taskMatchesView(task, name, now)).length}</span>
            </button>)}
          </nav>
          {addingTask ? <form className="task-quick-add" data-shortcut-scope="modal" onSubmit={(event) => { event.preventDefault(); void createTask(); }} onKeyDown={(event) => { if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); if (!creatingTask) setAddingTask(false); } }}>
            <label htmlFor="quick-add-task-title">Task title</label>
            <input id="quick-add-task-title" ref={newTaskInput} autoFocus value={newTaskTitle} onChange={(event) => setNewTaskTitle(event.target.value)} placeholder="What needs doing?" maxLength={240} />
            {!accountId && accountOptions.length > 1 ? <><label htmlFor="quick-add-task-account">Account</label><select id="quick-add-task-account" value={newTaskAccountId} onChange={(event) => setNewTaskAccountId(event.target.value)} required><option value="">Choose an account</option>{accountOptions.map((email) => <option key={email} value={email}>{email}</option>)}</select></> : null}
            <div><button type="submit" disabled={creatingTask || !newTaskTitle.trim() || (!accountId && accountOptions.length > 1 && !newTaskAccountId)}>{creatingTask ? "Adding…" : "Add task"}</button><button type="button" disabled={creatingTask} onClick={() => setAddingTask(false)}>Cancel</button></div>
          </form> : null}
          {!board && !loading && tasks.length > 0 && displayedGroups.length === 0 ? <p className="tasks-status">{view === "All" ? "No open tasks. Add a task or view completed work." : `No tasks in ${view.toLowerCase()}.`}</p> : null}
          {taskList}
        </div>
        <section id="task-detail-panel" className="task-detail" aria-label="Task details">
          {selectedTask ? <>
            <header>
              <div className="task-detail-heading">
                <span className="eyebrow">{selectedTask.kind.replace("_", " ")}{selectedTask.status === "in_progress" ? " · In progress" : ""}</span>
                <div className="task-detail-title-row">
                  <button type="button" className="task-detail-complete" aria-label={isActiveTaskStatus(selectedTask.status) ? `Complete ${selectedTask.title}` : `Reopen ${selectedTask.title}`} onClick={() => void setStatus(selectedTask, isActiveTaskStatus(selectedTask.status) ? "completed" : "open")}>
                    {isActiveTaskStatus(selectedTask.status) ? <Check size={20} /> : <RotateCcw size={20} />}
                  </button>
                  {editing === "title" ? <form className="task-inline-title" data-shortcut-scope="modal" onSubmit={(event) => { event.preventDefault(); if (titleDraft.trim()) void saveTask({ title: titleDraft.trim() }); }} onKeyDown={(event) => { if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); setEditing(null); } }}>
                    <input autoFocus aria-label="Task title" value={titleDraft} onChange={(event) => setTitleDraft(event.target.value)} maxLength={240} />
                    <button type="submit" disabled={saving || !titleDraft.trim()}>Save</button><button type="button" onClick={() => setEditing(null)}>Cancel</button>
                  </form> : <h2><button type="button" className="task-detail-title-button" aria-label={`Edit title: ${selectedTask.title}`} onClick={() => startEditing("title")}>{selectedTask.title}</button></h2>}
                </div>
              </div>
              <div className="task-detail-actions">
                {onEditTask ? <HoverTooltip label="Task options" placement="bottom">
                  <ActionButton label="Task Options" onClick={() => onEditTask(selectedTask)}><Pencil size={17} /></ActionButton>
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
              {editing === "due" ? <form data-shortcut-scope="modal" onSubmit={(event) => {
                event.preventDefault();
                if (!dueValueDraft) return;
                const trimmedZone = timeZoneDraft.trim();
                if (dueKindDraft === "datetime" && trimmedZone && !isValidTimeZone(trimmedZone)) {
                  setTimeZoneError("Choose a valid timezone, such as America/New_York.");
                  return;
                }
                void saveTask({
                  dueKind: dueKindDraft,
                  dueValue: dueKindDraft === "datetime" ? new Date(dueValueDraft).toISOString() : dueValueDraft,
                  timeZone: dueKindDraft === "datetime" ? (trimmedZone || Intl.DateTimeFormat().resolvedOptions().timeZone) : null,
                });
              }} onKeyDown={(event) => { if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); setEditing(null); } }}>
                <select aria-label="Due type" value={dueKindDraft} onChange={(event) => {
                  const nextKind = event.target.value as "date" | "datetime";
                  setDueKindDraft(nextKind);
                  setDueValueDraft((current) => convertDueInputValue(current, nextKind));
                }}><option value="date">Date</option><option value="datetime">Date and time</option></select>
                <input aria-label={dueKindDraft === "datetime" ? "Due date and time" : "Due date"} type={dueKindDraft === "datetime" ? "datetime-local" : "date"} value={dueValueDraft} onChange={(event) => setDueValueDraft(event.target.value)} required />
                {dueKindDraft === "datetime" ? <input
                  list="task-detail-timezones"
                  aria-label="Timezone"
                  aria-invalid={timeZoneError ? "true" : undefined}
                  value={timeZoneDraft}
                  onChange={(event) => { setTimeZoneDraft(event.target.value); setTimeZoneError(null); }}
                  onBlur={(event) => { const value = event.target.value.trim(); if (value && !isValidTimeZone(value)) setTimeZoneError("Choose a valid timezone, such as America/New_York."); }}
                  placeholder="America/New_York"
                /> : null}
                <datalist id="task-detail-timezones">{timeZones.map((zone) => <option key={zone} value={zone} />)}</datalist>
                {timeZoneError ? <p className="form-error" role="alert">{timeZoneError}</p> : null}
                <div className="task-inline-actions"><button type="submit" disabled={saving || !dueValueDraft}>Save</button>{selectedTask.dueKind !== "none" ? <button type="button" disabled={saving} onClick={() => void saveTask({ dueKind: "none", dueValue: null, timeZone: null })}>Clear date</button> : null}<button type="button" onClick={() => setEditing(null)}>Cancel</button></div>
              </form> : <button type="button" className={`task-detail-due-button${isOverdue(selectedTask) ? " task-due-overdue" : ""}`} onClick={() => startEditing("due")}>{formatDueDetail(selectedTask) ?? "Add a due date"}</button>}
              {selectedTask.repeatIntervalDays ? <p>Repeats every {selectedTask.repeatIntervalDays} days</p> : null}
            </section>
            {selectedTask.threadId ? <section className="task-detail-source" aria-label="Source conversation">
              <h3>Source conversation</h3>
              {selectedTask.subjectSnapshot ? <p>{selectedTask.subjectSnapshot}</p> : null}
              <button type="button" onClick={() => onOpenThread(selectedTask.threadId!)}><MessageSquare size={16} /> Open conversation</button>
              {selectedTask.evidenceText ? <details><summary>Source excerpt</summary><blockquote>{selectedTask.evidenceText}</blockquote></details> : null}
            </section> : null}
          </> : <p className="tasks-status">Select a task to see its details.</p>}
        </section>
      </div> : taskList}
    </section>
  );
});
