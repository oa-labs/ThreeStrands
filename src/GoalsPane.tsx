import { Pencil, Plus } from "lucide-react";
import { forwardRef, useImperativeHandle, useMemo, useRef } from "react";
import type { Goal, ThreadTask } from "./domain";
import { currentGoalGroups, formatPeriod, GOAL_HORIZON_LABELS, goalProgress } from "./goals";
import { isActiveTaskStatus } from "./taskViews";

/** Which tasks the workspace shows: all of them, those with no goal, or those supporting one goal. */
export type GoalFilter = null | { goalId: string | null };

export type GoalsPaneHandle = { focus(): void };

const STATUS_LABELS: Partial<Record<Goal["status"], string>> = { achieved: "Achieved", dropped: "Dropped" };

/** The ids of `goalId` and every goal that supports it, directly or through another goal. */
export function goalWithSupporters(goals: readonly Goal[], goalId: string): Set<string> {
  const ids = new Set([goalId]);
  for (let grew = true; grew;) {
    grew = false;
    for (const goal of goals) {
      if (goal.parentGoalId && ids.has(goal.parentGoalId) && !ids.has(goal.id)) {
        ids.add(goal.id);
        grew = true;
      }
    }
  }
  return ids;
}

/** Goals for the current quarter, half, and year beside the task workspace; selecting one filters the tasks. */
export const GoalsPane = forwardRef<GoalsPaneHandle, {
  goals: readonly Goal[];
  tasks: readonly ThreadTask[];
  filter: GoalFilter;
  now?: Date;
  onFilterChange(filter: GoalFilter): void;
  onAddGoal(): void;
  onEditGoal(goal: Goal): void;
  /** Escape from the pane hands focus back to the tasks. */
  onLeave(): void;
}>(function GoalsPane({ goals, tasks, filter, now = new Date(), onFilterChange, onAddGoal, onEditGoal, onLeave }, ref) {
  const paneRef = useRef<HTMLElement>(null);
  const groups = useMemo(() => currentGoalGroups(goals, now), [goals, now]);
  const shown = new Set(groups.flatMap((group) => group.goals.map((goal) => goal.id)));
  const otherGoals = goals.filter((goal) => !shown.has(goal.id));
  const byId = new Map(goals.map((goal) => [goal.id, goal]));
  const openTasks = tasks.filter((task) => isActiveTaskStatus(task.status));
  const unlinkedCount = openTasks.filter((task) => !task.goalId || !byId.has(task.goalId)).length;

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
    return <li key={goal.id} className={`goal-item goal-${goal.status}`}>
      <button type="button" className="goal-row" data-goal-row aria-pressed={selected({ goalId: goal.id })} onClick={() => choose({ goalId: goal.id })}>
        <span className="goal-title">{goal.title}</span>
        <span className="goal-meta">
          {showPeriod ? <span>{GOAL_HORIZON_LABELS[goal.horizon]} · {formatPeriod(goal.period)}</span> : null}
          {STATUS_LABELS[goal.status] ? <span className="goal-status">{STATUS_LABELS[goal.status]}</span> : null}
          <span>{progress.open} open · {progress.done} done</span>
          {parent ? <span className="goal-parent" title={`Supports ${parent.title}`}>↳ {parent.title}</span> : null}
        </span>
      </button>
      <button type="button" className="goal-edit" aria-label={`Edit goal ${goal.title}`} onClick={() => onEditGoal(goal)}><Pencil size={14} aria-hidden="true" /></button>
    </li>;
  };

  return (
    <aside ref={paneRef} id="goals-pane" className="goals-pane" aria-label="Goals" onKeyDown={onKeyDown}>
      <header className="goals-pane-header">
        <h2>Goals</h2>
        <button type="button" className="goal-add-button" onClick={onAddGoal}><Plus size={14} aria-hidden="true" />Add goal</button>
      </header>
      <ul className="goal-list goal-filters">
        <li><button type="button" className="goal-row" data-goal-row aria-pressed={filter === null} onClick={() => onFilterChange(null)}>
          <span className="goal-title">All tasks</span><span className="goal-meta">{openTasks.length} open</span>
        </button></li>
        <li><button type="button" className="goal-row" data-goal-row aria-pressed={selected({ goalId: null })} onClick={() => choose({ goalId: null })}>
          <span className="goal-title">No goal</span><span className="goal-meta">{unlinkedCount} open</span>
        </button></li>
      </ul>
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
