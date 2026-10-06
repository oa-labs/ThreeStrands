import { Check, ChevronLeft, ChevronRight, Clock3, Columns3, List, Mail, Plus, RotateCcw, Target, X } from "lucide-react";
import { forwardRef, useCallback, useEffect, useImperativeHandle, useMemo, useRef, useState, type CSSProperties } from "react";
import type { Goal, ThreadTask, UpdateGoalRequest, UpdateTaskRequest, TaskStatus } from "./domain";
import { mailClient } from "./data/client";
import { GoalDialog } from "./GoalDialog";
import { GoalLinkPicker } from "./GoalLinkPicker";
import { GoalReviewDialog } from "./GoalReviewDialog";
import { goalWithSupporters } from "./goals";
import { GoalsPane, type GoalFilter, type GoalsPaneHandle } from "./GoalsPane";
import { PanelResizeHandle, useGoalsPaneWidth } from "./PanelResizeHandle";
import { TaskDetailDialog } from "./TaskDetailDialog";
import { errorMessage } from "./errors";
import { adjacentTaskStatus, compareTasksForDisplay, formatDue, isActiveTaskStatus, isDue, isOverdue, TASK_BOARD_COLUMNS, TASK_VIEWS, taskBoardColumn, taskBoardColumnStatus, taskMatchesView, taskViewForAll, type TaskBoardColumn, type TaskView } from "./taskViews";

export type TaskLayout = "board" | "list";
const TASK_LAYOUT_KEY = "threestrands.tasks.layout";

function readTaskLayout(): TaskLayout {
  try {
    return localStorage.getItem(TASK_LAYOUT_KEY) === "list" ? "list" : "board";
  } catch {
    return "board";
  }
}

const TASK_VIEW_KEY = "threestrands.tasks.view";

function readTaskView(): TaskView {
  try {
    const stored = localStorage.getItem(TASK_VIEW_KEY);
    return TASK_VIEWS.find((name) => name === stored) ?? "All";
  } catch {
    return "All";
  }
}

const STATUS_LABELS: Record<TaskStatus, string> = { open: "To Do", in_progress: "In Progress", completed: "Done", cancelled: "Done" };
const TASK_CARD_KIND_LABELS: Partial<Record<ThreadTask["kind"], string>> = { follow_up: "Follow up", waiting_for: "Waiting" };

// The board's Done column already holds finished work, so it has no Completed view.
const BOARD_TASK_VIEWS = TASK_VIEWS.filter((name) => name !== "Completed");
const DRAG_THRESHOLD_PX = 5;

type CardDrag = {
  taskId: string;
  pointerId: number;
  startX: number;
  startY: number;
  offsetX: number;
  offsetY: number;
  width: number;
  x: number;
  y: number;
  active: boolean;
  over: TaskBoardColumn | null;
};

function boardColumnAt(x: number, y: number): TaskBoardColumn | null {
  const column = document.elementFromPoint?.(x, y)?.closest<HTMLElement>("[data-board-column]")?.dataset.boardColumn;
  return TASK_BOARD_COLUMNS.find((name) => name === column) ?? null;
}

const COMPLETED_RETENTION_MS = 3 * 24 * 60 * 60 * 1000;

function isStaleCompleted(task: ThreadTask): boolean {
  if (task.status !== "completed" || !task.completedAt) return false;
  return Date.now() - new Date(task.completedAt).getTime() > COMPLETED_RETENTION_MS;
}

export type TaskWorkspaceHandle = {
  startNew(): void;
  selectNext(): void;
  selectPrevious(): void;
  openSelected(): void;
  openDetails(): void;
  completeSelected(): void;
  reopenSelected(): void;
  moveSelected(direction: -1 | 1): void;
  selectAdjacentColumn(direction: -1 | 1): void;
  toggleLayout(): void;
  cycleView(direction: -1 | 1): void;
  focusGoals(): void;
  /** Opens the goal picker for the selected task. */
  linkSelectedToGoal(): void;
};

/** The Tasks workspace: a list or board of tasks beside the account's goals; each task opens in a detail dialog. */
export const TaskSidebar = forwardRef<TaskWorkspaceHandle, {
  accountId: string | null;
  accountOptions?: string[];
  onOpenThread(threadId: string): void;
  onTasksChanged?(): void;
  onDraftFollowUp?(task: ThreadTask): void;
  onCreateTask?(title: string, accountId?: string, goalId?: string): Promise<ThreadTask>;
  onSelectedTaskChange?(task: ThreadTask | null): void;
  onLayoutChange?(layout: TaskLayout): void;
  refreshKey?: number;
}>(function TaskSidebar({
  accountId,
  accountOptions = [],
  onOpenThread,
  onTasksChanged,
  onDraftFollowUp,
  onCreateTask,
  onSelectedTaskChange,
  onLayoutChange,
  refreshKey = 0,
}, ref) {
  const [tasks, setTasks] = useState<ThreadTask[]>([]);
  const [goals, setGoals] = useState<Goal[]>([]);
  const [goalFilter, setGoalFilter] = useState<GoalFilter>(null);
  const [goalDialog, setGoalDialog] = useState<null | { goalId: string | null }>(null);
  const [reviewingGoals, setReviewingGoals] = useState(false);
  const [goalLinkTaskId, setGoalLinkTaskId] = useState<string | null>(null);
  const goalsPane = useRef<GoalsPaneHandle>(null);
  const goalsPaneSize = useGoalsPaneWidth();
  const [now, setNow] = useState(() => new Date());
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [selectedTaskId, setSelectedTaskId] = useState<string | null>(null);
  const [view, setView] = useState<TaskView>(readTaskView);
  const [showOlderDone, setShowOlderDone] = useState(false);
  const [layout, setLayout] = useState<TaskLayout>(readTaskLayout);
  const board = layout === "board";
  const [detailTaskId, setDetailTaskId] = useState<string | null>(null);
  const detailOpen = useRef(false);
  detailOpen.current = detailTaskId !== null;
  const saveQueue = useRef<Promise<unknown>>(Promise.resolve());
  const [addingTask, setAddingTask] = useState(false);
  const [newTaskTitle, setNewTaskTitle] = useState("");
  const [newTaskAccountId, setNewTaskAccountId] = useState("");
  const [creatingTask, setCreatingTask] = useState(false);
  const [announcement, setAnnouncement] = useState("");
  const [completionToast, setCompletionToast] = useState<ThreadTask | null>(null);
  const completionToastTimeout = useRef<number | null>(null);
  const taskCards = useRef(new Map<string, HTMLElement>());
  const newTaskInput = useRef<HTMLInputElement>(null);

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
      const [nextTasks, nextGoals] = await Promise.all([
        mailClient.listTasks(accountId ?? undefined),
        mailClient.listGoals(accountId ?? undefined),
      ]);
      setTasks(nextTasks);
      setGoals(nextGoals);
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setLoading(false);
    }
  }, [accountId]);

  useEffect(() => { void load(); }, [load, refreshKey]);
  useEffect(() => {
    const interval = window.setInterval(() => setNow(new Date()), 60_000);
    return () => window.clearInterval(interval);
  }, []);
  const goalsById = useMemo(() => new Map(goals.map((goal) => [goal.id, goal])), [goals]);
  // A goal filter that no longer matches a goal (deleted, another account) falls back to all tasks.
  if (goalFilter?.goalId && !loading && !goalsById.has(goalFilter.goalId)) setGoalFilter(null);
  const filteredTasks = useMemo(() => {
    if (!goalFilter) return tasks;
    if (goalFilter.goalId === null) return tasks.filter((task) => !task.goalId || !goalsById.has(task.goalId));
    const supporting = goalWithSupporters(goals, goalFilter.goalId);
    return tasks.filter((task) => task.goalId && supporting.has(task.goalId));
  }, [goalFilter, goals, goalsById, tasks]);
  // An edited goal removed meanwhile (sync, another window) closes its dialog instead of turning it into "Add goal".
  if (goalDialog?.goalId && !loading && !goalsById.has(goalDialog.goalId)) setGoalDialog(null);
  const filterGoal = goalFilter?.goalId ? goalsById.get(goalFilter.goalId) ?? null : null;
  const sortedTasks = useMemo(() => [...filteredTasks].sort(compareTasksForDisplay), [filteredTasks]);
  const workspaceGroups = useMemo(() => TASK_VIEWS.filter((name) => name !== "All")
    .map((name) => ({ name, tasks: sortedTasks.filter((task) =>
      view === "All" ? taskViewForAll(task, now) === name && isActiveTaskStatus(task.status) : name === view && taskMatchesView(task, view, now),
    ) }))
    .filter((group) => group.tasks.length > 0), [now, sortedTasks, view]);
  const boardColumns = useMemo(() => TASK_BOARD_COLUMNS.map((name) => ({ name, tasks: sortedTasks.filter((task) =>
    taskBoardColumn(task.status) === name
    && (view === "All" ? showOlderDone || !isStaleCompleted(task) : taskMatchesView(task, view, now)),
  ) })), [now, showOlderDone, sortedTasks, view]);
  // A date or Waiting filter narrows open work, so the board drops the Done column rather than show it permanently empty.
  const visibleBoardColumns = view === "All" ? boardColumns : boardColumns.filter((column) => column.name !== "Done");
  const [cardDrag, setCardDrag] = useState<CardDrag | null>(null);
  const suppressCardClick = useRef(false);
  const olderDoneCount = useMemo(() => view === "All" ? filteredTasks.filter(isStaleCompleted).length : 0, [filteredTasks, view]);
  const displayedGroups = board ? boardColumns : workspaceGroups;
  const orderedTasks = useMemo(() => displayedGroups.flatMap((group) => group.tasks), [displayedGroups]);
  const selectedTask = orderedTasks.find((task) => task.id === selectedTaskId) ?? null;

  useEffect(() => {
    if (orderedTasks.length === 0) {
      setSelectedTaskId(null);
      return;
    }
    setSelectedTaskId((current) => current && orderedTasks.some((task) => task.id === current) ? current : orderedTasks[0].id);
  }, [orderedTasks]);

  useEffect(() => {
    if (selectedTaskId) taskCards.current.get(selectedTaskId)?.scrollIntoView?.({ block: "nearest" });
  }, [selectedTaskId]);

  useEffect(() => { onSelectedTaskChange?.(selectedTask); }, [onSelectedTaskChange, selectedTask]);
  useEffect(() => { onLayoutChange?.(layout); }, [layout, onLayoutChange]);
  useEffect(() => { if (board && view === "Completed") setView("All"); }, [board, view]);
  useEffect(() => {
    try {
      localStorage.setItem(TASK_VIEW_KEY, view);
    } catch {
      // The chosen view still applies for this session.
    }
  }, [view]);

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

  // Saves run one at a time so a slower earlier response cannot overwrite a later edit.
  const updateTask = useCallback((taskId: string, request: Omit<UpdateTaskRequest, "id">) => {
    const save = saveQueue.current.then(async () => {
      const updated = await mailClient.updateTask({ id: taskId, ...request });
      setTasks((current) => current.map((task) => task.id === updated.id ? updated : task));
      onTasksChanged?.();
    });
    saveQueue.current = save.catch(() => undefined);
    return save.catch((reason) => {
      // The dialog reports its own failures; once it has closed, the banner does.
      if (!detailOpen.current) setError(errorMessage(reason));
      throw reason;
    });
  }, [onTasksChanged]);

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
    if (!onCreateTask || !title || creatingTask || (!accountId && !filterGoal && accountOptions.length > 1 && !newTaskAccountId)) return;
    setCreatingTask(true);
    setError(null);
    try {
      // Adding while one goal is in view links the task to it, in the goal's account.
      const created = filterGoal ? await onCreateTask(title, filterGoal.accountId, filterGoal.id)
        : accountId ? await onCreateTask(title) : await onCreateTask(title, newTaskAccountId || accountOptions[0]);
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

  // Like task saves, goal saves run one at a time in the order they were made.
  const updateGoal = useCallback((goalId: string, request: Omit<UpdateGoalRequest, "id">) => {
    const save = saveQueue.current.then(async () => {
      const updated = await mailClient.updateGoal({ id: goalId, ...request });
      setGoals((current) => current.map((goal) => goal.id === updated.id ? updated : goal));
    });
    saveQueue.current = save.catch(() => undefined);
    return save;
  }, []);

  const adjacentTaskId = useCallback((direction: -1 | 1): string | null => {
    if (orderedTasks.length === 0) return null;
    if (board) {
      // On the board, up and down stay inside the selected card's column and stop at its ends.
      const column = boardColumns.find((candidate) => candidate.tasks.some((task) => task.id === selectedTaskId));
      if (!column) return orderedTasks[0].id;
      const index = column.tasks.findIndex((task) => task.id === selectedTaskId);
      return column.tasks[Math.max(0, Math.min(column.tasks.length - 1, index + direction))].id;
    }
    const currentIndex = orderedTasks.findIndex((task) => task.id === selectedTaskId);
    const from = currentIndex === -1 ? 0 : currentIndex;
    return orderedTasks[(from + direction + orderedTasks.length) % orderedTasks.length].id;
  }, [board, boardColumns, orderedTasks, selectedTaskId]);

  const moveSelection = useCallback((direction: -1 | 1) => {
    const next = adjacentTaskId(direction);
    if (next) setSelectedTaskId(next);
  }, [adjacentTaskId]);

  const openDetails = useCallback((taskId: string) => {
    setSelectedTaskId(taskId);
    setDetailTaskId(taskId);
  }, []);

  useEffect(() => {
    if (!cardDrag?.active) return;
    const cancel = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      event.stopPropagation();
      setCardDrag(null);
    };
    window.addEventListener("keydown", cancel, true);
    return () => window.removeEventListener("keydown", cancel, true);
  }, [cardDrag?.active]);

  const dropCard = (drag: CardDrag) => {
    const task = tasks.find((candidate) => candidate.id === drag.taskId);
    if (!task || !drag.over || drag.over === taskBoardColumn(task.status)) return;
    if (!visibleBoardColumns.some((column) => column.name === drag.over)) return;
    setSelectedTaskId(task.id);
    void setStatus(task, taskBoardColumnStatus(drag.over));
  };

  useImperativeHandle(ref, () => ({
    startNew,
    selectNext: () => moveSelection(1),
    selectPrevious: () => moveSelection(-1),
    openSelected: () => {
      if (selectedTask?.threadId) onOpenThread(selectedTask.threadId);
    },
    openDetails: () => { if (selectedTask) openDetails(selectedTask.id); },
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
    toggleLayout: () => changeLayout(layout === "board" ? "list" : "board"),
    focusGoals: () => goalsPane.current?.focus(),
    linkSelectedToGoal: () => { if (selectedTask) setGoalLinkTaskId(selectedTask.id); },
    cycleView: (direction) => {
      const views: readonly TaskView[] = board ? BOARD_TASK_VIEWS : TASK_VIEWS;
      setView((current) => views[(Math.max(0, views.indexOf(current)) + direction + views.length) % views.length]);
    },
  }), [board, boardColumns, changeLayout, layout, moveSelection, onOpenThread, openDetails, selectedTask, setStatus, startNew]);

  const renderCard = (task: ThreadTask) => {
    const active = isActiveTaskStatus(task.status);
    const back = board ? adjacentTaskStatus(task.status, -1) : null;
    const forward = board ? adjacentTaskStatus(task.status, 1) : null;
    const due = formatDue(task);
    const kindLabel = TASK_CARD_KIND_LABELS[task.kind];
    const cardGoal = task.goalId ? goalsById.get(task.goalId) ?? null : null;
    const followUp = onDraftFollowUp && task.threadId && task.kind === "follow_up" && isDue(task) ? (
      <button type="button" className="btn btn-sm task-follow-up-button" onClick={() => onDraftFollowUp(task)}>Draft Follow-Up</button>
    ) : null;
    return <article
      id={`task-${task.id}`}
      ref={(node) => { if (node) taskCards.current.set(task.id, node); else taskCards.current.delete(task.id); }}
      className={`task-card task-${task.status}${selectedTaskId === task.id ? " selected" : ""}${isOverdue(task) ? " task-card-overdue" : ""}${cardDrag?.active && cardDrag.taskId === task.id ? " dragging" : ""}`}
      aria-current={selectedTaskId === task.id ? "true" : undefined}
      key={task.id}
    >
      <button
        type="button"
        className="task-card-main"
        onClick={() => {
          if (suppressCardClick.current) {
            suppressCardClick.current = false;
            return;
          }
          openDetails(task.id);
        }}
        onPointerDown={board ? (event) => {
          if (event.button !== 0) return;
          const rect = event.currentTarget.closest("article")?.getBoundingClientRect();
          suppressCardClick.current = false;
          setCardDrag({
            taskId: task.id, pointerId: event.pointerId, startX: event.clientX, startY: event.clientY,
            offsetX: rect ? event.clientX - rect.left : 0, offsetY: rect ? event.clientY - rect.top : 0, width: rect?.width ?? 0,
            x: event.clientX, y: event.clientY, active: false, over: null,
          });
        } : undefined}
        onPointerMove={board ? (event) => {
          const drag = cardDrag;
          if (!drag || drag.pointerId !== event.pointerId || drag.taskId !== task.id) return;
          if (!drag.active && Math.hypot(event.clientX - drag.startX, event.clientY - drag.startY) < DRAG_THRESHOLD_PX) return;
          if (!drag.active) event.currentTarget.setPointerCapture?.(event.pointerId);
          setCardDrag({ ...drag, active: true, x: event.clientX, y: event.clientY, over: boardColumnAt(event.clientX, event.clientY) });
        } : undefined}
        onPointerUp={board ? (event) => {
          const drag = cardDrag;
          if (!drag || drag.pointerId !== event.pointerId) return;
          setCardDrag(null);
          if (!drag.active) return;
          suppressCardClick.current = true;
          event.currentTarget.releasePointerCapture?.(event.pointerId);
          dropCard({ ...drag, over: boardColumnAt(event.clientX, event.clientY) });
        } : undefined}
        onPointerCancel={board ? () => setCardDrag(null) : undefined}
      >
        <strong>{task.title}</strong>
        {!board && task.status === "in_progress" ? <span className="task-progress-badge">In progress</span> : null}
        {due || kindLabel || task.threadId || task.status === "cancelled" || cardGoal ? <span className="task-card-meta">
          {task.status === "cancelled" ? <small className="task-card-kind">Cancelled</small> : null}
          {due ? <small className={isOverdue(task) ? "task-due-overdue" : undefined}><Clock3 size={12} /> {due}</small> : null}
          {kindLabel ? <small className="task-card-kind">{kindLabel}</small> : null}
          {task.threadId ? <small className="task-card-source" title="From an email"><Mail size={12} aria-hidden="true" /><span className="sr-only">From an email</span></small> : null}
          {cardGoal ? <small className="task-card-goal" title={`Supports ${cardGoal.title}`}><Target size={12} aria-hidden="true" /><span className="sr-only">Supports </span>{cardGoal.title}</small> : null}
        </span> : null}
      </button>
      {board ? (back || forward || followUp) ? <div className="task-board-moves">
        {followUp}
        {back ? <button type="button" className="btn btn-sm" aria-label={`Move ${task.title} to ${STATUS_LABELS[back]}`} onClick={() => void setStatus(task, back)}><ChevronLeft size={14} aria-hidden="true" />{STATUS_LABELS[back]}</button> : null}
        {forward ? <button type="button" className="btn btn-sm" aria-label={`Move ${task.title} to ${STATUS_LABELS[forward]}`} onClick={() => void setStatus(task, forward)}>{STATUS_LABELS[forward]}<ChevronRight size={14} aria-hidden="true" /></button> : null}
      </div> : null : <>
        <button type="button" className="btn-icon btn-icon-sm task-status-button" aria-label={active ? `Complete ${task.title}` : `Reopen ${task.title}`} onClick={() => void setStatus(task, active ? "completed" : "open")}>
          {active ? <Check size={15} /> : <RotateCcw size={15} />}
        </button>
        {followUp}
      </>}
    </article>;
  };

  const detailTask = detailTaskId ? tasks.find((task) => task.id === detailTaskId) ?? null : null;
  const goalLinkTask = goalLinkTaskId ? tasks.find((task) => task.id === goalLinkTaskId) ?? null : null;
  // A reload that drops the open task (another account, a sync delete) closes its dialog.
  if (detailTaskId && !loading && !detailTask) setDetailTaskId(null);
  const detailColumn = board ? boardColumns.find((column) => column.tasks.some((task) => task.id === detailTaskId))?.tasks : orderedTasks;
  const detailIndex = detailColumn?.findIndex((task) => task.id === detailTaskId) ?? -1;
  const detailPosition = detailColumn && detailIndex !== -1 ? { index: detailIndex, total: detailColumn.length } : null;

  const draggedTask = cardDrag?.active ? tasks.find((task) => task.id === cardDrag.taskId) ?? null : null;
  const taskList = board ? <div className="task-board" style={{ "--task-board-columns": visibleBoardColumns.length } as CSSProperties}>
    {visibleBoardColumns.map((column) => {
      const headingId = `task-column-${column.name.replace(/\s/g, "-")}`;
      const dropTarget = draggedTask && cardDrag?.over === column.name && taskBoardColumn(draggedTask.status) !== column.name;
      return <section key={column.name} className={`task-board-column${dropTarget ? " drop-target" : ""}`} aria-labelledby={headingId} data-board-column={column.name}>
        <header><h3 id={headingId}>{column.name}</h3><span className="task-view-count">{column.tasks.length}</span></header>
        <div className="task-board-cards">
          {column.tasks.length > 0 ? column.tasks.map(renderCard) : olderDoneCount > 0 && column.name === "Done" && !showOlderDone ? null : <p className="task-board-empty">No tasks</p>}
          {column.name === "Done" && olderDoneCount > 0 ? <button type="button" className="btn btn-sm btn-ghost task-board-older" aria-expanded={showOlderDone} onClick={() => setShowOlderDone((shown) => !shown)}>
            {showOlderDone ? "Hide older completed" : `Show ${olderDoneCount} older completed`}
          </button> : null}
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

  return (
    <section className="tasks-workspace" aria-label="Tasks">
      <header className="tasks-sidebar-header">
        <div className="thread-header-title">
          <div>
            <span className="eyebrow">
              Tasks
              <span className="eyebrow-account"> · {accountId ?? "All accounts"}</span>
            </span>
            <h1>{orderedTasks.length} {orderedTasks.length === 1 ? "task" : "tasks"}</h1>
            {goalFilter ? <button type="button" className="task-goal-filter-chip" aria-label={`Show all tasks, not only ${filterGoal ? `those supporting ${filterGoal.title}` : "those with no goal"}`} onClick={() => setGoalFilter(null)}>
              <Target size={12} aria-hidden="true" /><span>{filterGoal ? `Supports ${filterGoal.title}` : "No goal"}</span><X size={12} aria-hidden="true" />
            </button> : null}
          </div>
        </div>
        <div className="tasks-sidebar-header-actions">
          <div className="segmented" role="group" aria-label="Task layout">
            <button type="button" className="segment" aria-pressed={layout === "list"} onClick={() => changeLayout("list")}><List size={15} />List</button>
            <button type="button" className="segment" aria-pressed={layout === "board"} onClick={() => changeLayout("board")}><Columns3 size={15} />Board</button>
          </div>
          {onCreateTask ? <button type="button" className="btn task-add-button" onClick={startNew}><Plus size={15} />Add Task</button> : null}
        </div>
      </header>
      {error ? <p className="form-error tasks-error" role="alert">
        <span>{error}</span>
        <button type="button" className="btn-icon btn-icon-sm" aria-label="Dismiss error" onClick={() => setError(null)}><X size={13} /></button>
      </p> : null}
      <p className="sr-only" role="status" aria-live="polite">{announcement}</p>
      {completionToast ? (
        <div className="toast task-complete-toast" role="status">
          Completed &ldquo;{completionToast.title}&rdquo;
          <button type="button" className="btn-link" onClick={() => void setStatus(completionToast, isActiveTaskStatus(completionToast.status) ? completionToast.status : "open")}>Undo</button>
        </div>
      ) : null}
      {loading ? <p className="tasks-status">Loading tasks…</p> : null}
      {!loading && tasks.length === 0 && !addingTask ? <p className="tasks-status">No tasks yet. Press d to add one.</p> : null}
      <div className={`tasks-workspace-body${board ? " tasks-board-layout" : ""}`} style={{ "--goals-pane-width": `${goalsPaneSize.width}px` } as CSSProperties}>
        <div className={board ? "tasks-board-pane" : "tasks-list-pane"}>
          <nav className="task-view-nav" aria-label="Task views">
            {(board ? BOARD_TASK_VIEWS : TASK_VIEWS).map((name) => {
              const count = filteredTasks.filter((task) => taskMatchesView(task, name, now)).length;
              return <button key={name} type="button" className="task-view-item" aria-pressed={view === name} onClick={() => setView(name)}>
                <span>{name}</span>{count > 0 ? <span className="task-view-count" aria-hidden="true">{count}</span> : null}
              </button>;
            })}
          </nav>
          {addingTask ? <form className="task-quick-add" data-shortcut-scope="modal" onSubmit={(event) => { event.preventDefault(); void createTask(); }} onKeyDown={(event) => { if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); if (!creatingTask) setAddingTask(false); } }}>
            <label htmlFor="quick-add-task-title">Task title</label>
            <input id="quick-add-task-title" ref={newTaskInput} autoFocus value={newTaskTitle} onChange={(event) => setNewTaskTitle(event.target.value)} placeholder="What needs doing?" maxLength={240} />
            {filterGoal ? <p className="task-quick-add-goal"><Target size={12} aria-hidden="true" /> Supports {filterGoal.title}</p> : null}
            {!accountId && !filterGoal && accountOptions.length > 1 ? <><label htmlFor="quick-add-task-account">Account</label><select id="quick-add-task-account" value={newTaskAccountId} onChange={(event) => setNewTaskAccountId(event.target.value)} required><option value="">Choose an account</option>{accountOptions.map((email) => <option key={email} value={email}>{email}</option>)}</select></> : null}
            <div><button type="button" className="btn" disabled={creatingTask} onClick={() => setAddingTask(false)}>Cancel</button><button type="submit" className="btn btn-primary" disabled={creatingTask || !newTaskTitle.trim() || (!accountId && !filterGoal && accountOptions.length > 1 && !newTaskAccountId)}>{creatingTask ? "Adding…" : "Add task"}</button></div>
          </form> : null}
          {!board && !loading && tasks.length > 0 && displayedGroups.length === 0 ? <p className="tasks-status">{view === "All" ? "No open tasks. Add a task or view completed work." : `No tasks in ${view.toLowerCase()}.`}</p> : null}
          {taskList}
          {draggedTask && cardDrag ? <div className="task-drag-preview" aria-hidden="true" style={{ left: cardDrag.x - cardDrag.offsetX, top: cardDrag.y - cardDrag.offsetY, width: cardDrag.width }}>
            <strong>{draggedTask.title}</strong>
          </div> : null}
          <PanelResizeHandle {...goalsPaneSize} panelSide="right" label="Resize goals" controlsId="goals-pane" title="Drag to resize the goals. Use arrow keys to adjust; double-click to reset." />
        </div>
        <GoalsPane
          ref={goalsPane}
          goals={goals}
          tasks={tasks}
          filter={goalFilter}
          onFilterChange={setGoalFilter}
          onAddGoal={() => setGoalDialog({ goalId: null })}
          onEditGoal={(goal) => setGoalDialog({ goalId: goal.id })}
          onReview={() => setReviewingGoals(true)}
          onLeave={() => (selectedTaskId ? taskCards.current.get(selectedTaskId)?.querySelector<HTMLElement>(".task-card-main") : null)?.focus()}
        />
      </div>
      {reviewingGoals ? <GoalReviewDialog
        goals={goals}
        tasks={tasks}
        onUpdate={updateGoal}
        onClose={() => setReviewingGoals(false)}
      /> : null}
      {goalLinkTask ? <GoalLinkPicker
        task={goalLinkTask}
        goals={goals}
        onLink={(goalId) => updateTask(goalLinkTask.id, { goalId })}
        onCreateAndLink={async (title, period) => {
          const created = await mailClient.createGoal({ accountId: goalLinkTask.accountId, title, horizon: "quarter", period });
          setGoals((current) => [...current, created]);
          await updateTask(goalLinkTask.id, { goalId: created.id });
        }}
        onClose={() => setGoalLinkTaskId(null)}
      /> : null}
      {goalDialog ? <GoalDialog
        goal={goalDialog.goalId ? goalsById.get(goalDialog.goalId) ?? null : null}
        goals={goals}
        accountOptions={accountOptions}
        defaultAccountId={accountId}
        onCreate={async (request) => {
          const created = await mailClient.createGoal(request);
          setGoals((current) => [...current, created]);
          setGoalDialog(null);
          setGoalFilter({ goalId: created.id });
        }}
        onUpdate={(request) => updateGoal(goalDialog.goalId!, request)}
        onDelete={async () => {
          const goalId = goalDialog.goalId!;
          await mailClient.deleteGoal(goalId);
          setGoalDialog(null);
          await load();
          onTasksChanged?.();
        }}
        onClose={() => setGoalDialog(null)}
      /> : null}
      {detailTask ? <TaskDetailDialog
        task={detailTask}
        goals={goals}
        position={detailPosition}
        onUpdate={(request) => updateTask(detailTask.id, request)}
        onSetStatus={(status) => void setStatus(detailTask, status)}
        onNavigate={(direction) => {
          const next = adjacentTaskId(direction);
          if (next) openDetails(next);
        }}
        onOpenThread={onOpenThread}
        onClose={() => setDetailTaskId(null)}
      /> : null}
    </section>
  );
});
