import type { Goal, GoalHorizon, ThreadTask } from "./domain";
import { isActiveTaskStatus } from "./taskViews";

export const GOAL_HORIZONS: readonly GoalHorizon[] = ["quarter", "half", "year"];
export const GOAL_HORIZON_LABELS: Record<GoalHorizon, string> = { year: "Year", half: "Half", quarter: "Quarter" };

/** The period of `horizon` that contains `date`: `2026`, `2026-H2`, or `2026-Q4`. */
export function periodFor(horizon: GoalHorizon, date = new Date()): string {
  const year = date.getFullYear();
  const month = date.getMonth();
  if (horizon === "year") return `${year}`;
  if (horizon === "half") return `${year}-H${month < 6 ? 1 : 2}`;
  return `${year}-Q${Math.floor(month / 3) + 1}`;
}

/** Whether `period` names one period of `horizon`. Mirrors the sync contract. */
export function goalPeriodMatches(horizon: GoalHorizon, period: string): boolean {
  const pattern = horizon === "year" ? /^\d{4}$/ : horizon === "half" ? /^\d{4}-H[12]$/ : /^\d{4}-Q[1-4]$/;
  return pattern.test(period);
}

/** Readable period name: "2026", "H2 2026", "Q4 2026". */
export function formatPeriod(period: string): string {
  const [year, part] = period.split("-");
  return part ? `${part} ${year}` : year;
}

/** The adjacent period of the same horizon, for stepping a period picker. */
export function shiftPeriod(horizon: GoalHorizon, period: string, direction: -1 | 1): string {
  const [yearText, part] = period.split("-");
  const year = Number(yearText);
  if (horizon === "year") return `${year + direction}`;
  const count = horizon === "half" ? 2 : 4;
  const index = Number(part.slice(1)) - 1 + direction;
  const wrapped = (index + count) % count;
  const nextYear = year + Math.floor(index / count);
  return `${nextYear}-${horizon === "half" ? "H" : "Q"}${wrapped + 1}`;
}

/** Longer horizons rank lower; a goal can support only a lower-ranked one. */
function horizonRank(horizon: GoalHorizon): number {
  return horizon === "year" ? 0 : horizon === "half" ? 1 : 2;
}

/** Whether the period `outer` (of a longer horizon) contains `inner`. */
export function periodEncloses(outer: string, inner: string): boolean {
  const [outerYear, outerPart] = outer.split("-");
  const [innerYear, innerPart] = inner.split("-");
  if (outerYear !== innerYear || !innerPart) return false;
  if (!outerPart) return true;
  return outerPart === "H1" ? innerPart === "Q1" || innerPart === "Q2" : outerPart === "H2" && (innerPart === "Q3" || innerPart === "Q4");
}

/** The goals `goal` may support: same account, longer horizon, enclosing period. */
export function parentCandidates(goals: readonly Goal[], goal: Pick<Goal, "id" | "accountId" | "horizon" | "period">): Goal[] {
  return goals.filter((candidate) =>
    candidate.id !== goal.id
    && candidate.accountId === goal.accountId
    && horizonRank(candidate.horizon) < horizonRank(goal.horizon)
    && periodEncloses(candidate.period, goal.period));
}

/** The goals a task may support: active goals in the task's account, plus its current goal. */
export function goalOptionsForTask(goals: readonly Goal[], task: Pick<ThreadTask, "accountId" | "goalId">): Goal[] {
  return goals.filter((goal) => goal.accountId === task.accountId && (goal.status === "active" || goal.id === task.goalId));
}

export type GoalProgress = { open: number; done: number };

/** Linked task counts, never a percentage: tasks keep arriving, so a ratio would mislead. */
export function goalProgress(goal: Goal, tasks: readonly ThreadTask[]): GoalProgress {
  const linked = tasks.filter((task) => task.goalId === goal.id);
  const open = linked.filter((task) => isActiveTaskStatus(task.status)).length;
  return { open, done: linked.length - open };
}

/** Goals for the current period of each horizon, shortest horizon first. */
export function currentGoalGroups(goals: readonly Goal[], now = new Date()): { horizon: GoalHorizon; period: string; goals: Goal[] }[] {
  return GOAL_HORIZONS.map((horizon) => {
    const period = periodFor(horizon, now);
    return { horizon, period, goals: goals.filter((goal) => goal.horizon === horizon && goal.period === period) };
  });
}
