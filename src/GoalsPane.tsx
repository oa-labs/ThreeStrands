import { Pencil, Plus } from "lucide-react";
import { forwardRef, useImperativeHandle, useMemo, useRef, useState } from "react";
import type { Goal, ThreadTask } from "./domain";
import { currentGoalGroups, formatPeriod, GOAL_HORIZON_LABELS, GOAL_STALE_AFTER_DAYS, goalIsStale, goalProgress, goalsToReview, periodFor } from "./goals";
import { isActiveTaskStatus } from "./taskViews";

/** Which tasks the workspace shows: all of them, those with no goal, or those supporting one goal. */
export type GoalFilter = null | { goalId: string | null };

export type GoalsPaneHandle = { focus(): void };

/** An account other than the current one and how many goals it has. */
export type OtherAccountGoals = { accountId: string; count: number };

const STATUS_LABELS: Partial<Record<Goal["status"], string>> = { achieved: "Achieved", dropped: "Dropped" };
const REVIEW_DEFERRED_KEY = "threestrands.goals.reviewDeferredUntil";

function readReviewDeferral(): string | null {
  try {
    return localStorage.getItem(REVIEW_DEFERRED_KEY);
  } catch {
    return null;
  }
}

/** Goals for the current quarter, half, and year beside the task workspace; selecting one filters the tasks. */
export const GoalsPane = forwardRef<GoalsPaneHandle, {
  goals: readonly Goal[];
  /** The account whose goals are shown; null shows every account's goals. */
  accountId?: string | null;
  /** Accounts that have goals, named when the current account has none. */
  otherAccountGoals?: readonly OtherAccountGoals[];
  tasks: readonly ThreadTask[];
  filter: GoalFilter;
  now?: Date;
  onFilterChange(filter: GoalFilter): void;
  onAddGoal(): void;
  onEditGoal(goal: Goal): void;
  /** Opens the review of goals whose period has ended. */
  onReview(): void;
  /** Escape from the pane hands focus back to the tasks. */
  onLeave(): void;
}>(function GoalsPane({ goals, accountId = null, otherAccountGoals = [], tasks, filter, now = new Date(), onFilterChange, onAddGoal, onEditGoal, onReview, onLeave }, ref) {
  const paneRef = useRef<HTMLElement>(null);
  const groups = useMemo(() => currentGoalGroups(goals, now), [goals, now]);
  const shown = new Set(groups.flatMap((group) => group.goals.map((goal) => goal.id)));
  const otherGoals = goals.filter((goal) => !shown.has(goal.id));
  const byId = new Map(goals.map((goal) => [goal.id, goal]));
  const openTasks = tasks.filter((task) => isActiveTaskStatus(task.status));
  const unlinkedCount = openTasks.filter((task) => !task.goalId || !byId.has(task.goalId)).length;

  const toReview = goalsToReview(goals, now);
  // "Later" hides the prompt for the rest of the current quarter on this device.
  const currentQuarter = periodFor("quarter", now);
  const [deferredUntil, setDeferredUntil] = useState(readReviewDeferral);
  const deferReview = () => {
    setDeferredUntil(currentQuarter);
    try {
      localStorage.setItem(REVIEW_DEFERRED_KEY, currentQuarter);
    } catch {
      // The prompt stays hidden for this session.
    }
  };

  const rows = () => [...(paneRef.current?.querySelectorAll<HTMLElement>("[data-goal-row]") ?? [])];
  useImperativeHandle(ref, () => ({
    focus: () => (rows().find((row) => row.getAttribute("aria-pressed") === "true") ?? rows()[0])?.focus(),
  }));

  const onKeyDown = (event: React.KeyboardEvent) => {
    if (event.metaKey || event.ctrlKey || event.altKey) return;
    const list = rows();
    const index = list.indexOf(document.activeElement as HTMLElement);
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      onLeave();
    } else if ((event.key === "j" || event.key === "ArrowDown" || event.key === "k" || event.key === "ArrowUp") && index !== -1) {
      // Claimed here so the workspace's own j/k does not move the task selection.
      event.preventDefault();
      const step = event.key === "j" || event.key === "ArrowDown" ? 1 : -1;
      list[Math.max(0, Math.min(list.length - 1, index + step))]?.focus();
    }
  };

  const selected = (candidate: GoalFilter) => JSON.stringify(candidate) === JSON.stringify(filter);
  const choose = (candidate: GoalFilter) => onFilterChange(selected(candidate) ? null : candidate);

  const renderGoal = (goal: Goal, showPeriod = false) => {
    const progress = goalProgress(goal, tasks);
    const parent = goal.parentGoalId ? byId.get(goal.parentGoalId) : null;
    const stale = goalIsStale(goal, goals, tasks, now);
    return <li key={goal.id} className={`goal-item goal-${goal.status}`}>
      <button type="button" className="goal-row" data-goal-row aria-pressed={selected({ goalId: goal.id })} onClick={() => choose({ goalId: goal.id })}>
        <span className="goal-title">{goal.title}</span>
        <span className="goal-meta">
          {showPeriod ? <span>{GOAL_HORIZON_LABELS[goal.horizon]} · {formatPeriod(goal.period)}</span> : null}
          {stale ? <span className="goal-stale" title={`No task activity in ${GOAL_STALE_AFTER_DAYS} days`}>Stale</span> : null}
          {STATUS_LABELS[goal.status] ? <span className="goal-status">{STATUS_LABELS[goal.status]}</span> : null}
          <span>{progress.open} open · {progress.done} done</span>
          {parent ? <span className="goal-parent" title={`Supports ${parent.title}`}>↳ {parent.title}</span> : null}
        </span>
      </button>
      <button type="button" className="btn-icon btn-icon-sm goal-edit" aria-label={`Edit goal ${goal.title}`} onClick={() => onEditGoal(goal)}><Pencil size={14} aria-hidden="true" /></button>
    </li>;
  };

  return (
    <aside ref={paneRef} id="goals-pane" className="goals-pane" aria-label="Goals" onKeyDown={onKeyDown}>
      <header className="goals-pane-header">
        <h2>Goals</h2>
        <button type="button" className="btn btn-sm" onClick={onAddGoal}><Plus size={14} aria-hidden="true" />Add goal</button>
      </header>
      {toReview.length && deferredUntil !== currentQuarter ? <div className="goal-review-prompt" role="status">
        <span>{toReview.length === 1 ? "1 goal" : `${toReview.length} goals`} from a past period {toReview.length === 1 ? "needs" : "need"} review.</span>
        <span className="goal-review-actions">
          <button type="button" className="btn btn-sm btn-primary" onClick={onReview}>Review</button>
          <button type="button" className="btn btn-sm" onClick={deferReview}>Later</button>
        </span>
      </div> : null}
      <ul className="goal-list goal-filters">
        <li><button type="button" className="goal-row" data-goal-row aria-pressed={filter === null} onClick={() => onFilterChange(null)}>
          <span className="goal-title">All tasks</span><span className="goal-meta">{openTasks.length} open</span>
        </button></li>
        <li><button type="button" className="goal-row" data-goal-row aria-pressed={selected({ goalId: null })} onClick={() => choose({ goalId: null })}>
          <span className="goal-title">No goal</span><span className="goal-meta">{unlinkedCount} open</span>
        </button></li>
      </ul>
      {accountId && goals.length === 0 ? <div className="goal-account-note" role="note">
        <p>Goals belong to each account. {accountId} has no goals yet.</p>
        {otherAccountGoals.length ? <p>Goals on other accounts: {otherAccountGoals.map((other, index) => <span key={other.accountId}>
          {index ? ", " : null}<strong>{other.accountId}</strong> ({other.count})
        </span>)}.</p> : null}
      </div> : null}
      {groups.map((group) => group.goals.length === 0 && group.horizon === "half" ? null : <section key={group.horizon} className="goal-group" aria-label={`${GOAL_HORIZON_LABELS[group.horizon]} goals`}>
        <h3>{GOAL_HORIZON_LABELS[group.horizon]} · {formatPeriod(group.period)}</h3>
        {group.goals.length ? <ul className="goal-list">{group.goals.map((goal) => renderGoal(goal))}</ul> : <p className="goal-empty">No {GOAL_HORIZON_LABELS[group.horizon].toLowerCase()} goals yet.</p>}
      </section>)}
      {otherGoals.length ? <details className="goal-group goal-other">
        <summary>Other periods ({otherGoals.length})</summary>
        <ul className="goal-list">{otherGoals.map((goal) => renderGoal(goal, true))}</ul>
      </details> : null}
    </aside>
  );
});
