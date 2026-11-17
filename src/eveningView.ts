import { newOperationId } from "./planRoutine";
import type { DailyPlan, DailyPlanItem, NotTodayEntry, PlanChanges, PlanReview, PlanReviewItem } from "./domain";
import type { DecisionKind } from "./PlanReviewColumns";
import { readLocalStorage, writeLocalStorage } from "./useLocalStorageState";

/** Today's two faces: the plan for the day, and the evening review of it with tomorrow beside it. */
export type TodayView = "day" | "evening";

export const TODAY_VIEW_KEY = "threestrands.today.view";

type StoredView = { day: string; view: TodayView };

/**
 * The view chosen with Review Today or Back to Today's Plan on `day`, kept
 * on this device for the rest of that day only; null once the day has turned
 * over or when nothing was chosen, so the time of day decides.
 */
export function readViewChoice(day: string): TodayView | null {
  return readLocalStorage<TodayView | null>(TODAY_VIEW_KEY, (raw) => {
    const stored = raw ? (JSON.parse(raw) as Partial<StoredView>) : null;
    return stored?.day === day && (stored.view === "day" || stored.view === "evening") ? stored.view : null;
  }, null);
}

export function writeViewChoice(day: string, view: TodayView): void {
  writeLocalStorage(TODAY_VIEW_KEY, JSON.stringify({ day, view } satisfies StoredView));
}

/** The chosen view, else the evening view once it has opened by itself. */
export function resolveView(choice: TodayView | null, opened: boolean): TodayView {
  return choice ?? (opened ? "evening" : "day");
}

/** The review describes today only when today's own plan is the most recent plan it found. */
export const reviewsToday = (review: PlanReview | null, today: string): boolean => review !== null && review.reviewed === today;

/**
 * The loose ends among the review's Didn't get to rows: everything not yet
 * decided in today's note. The review is read for tomorrow, so an item carried
 * to tomorrow (a deferral to tomorrow's date, which is due then) still comes
 * back as due; today's own Not today lines say it is decided.
 */
export function looseEnds(review: PlanReview | null, plan: DailyPlan | null): PlanReviewItem[] {
  if (!review) return [];
  const decided = new Set((plan?.notToday ?? []).map((item) => item.key));
  return review.didntGetTo.filter((row) => !decided.has(row.key));
}

/** Today's items carried to tomorrow: Not today lines deferred to tomorrow's date. */
export function carriedToTomorrow(plan: DailyPlan | null, tomorrow: string): DailyPlanItem[] {
  return (plan?.notToday ?? []).filter((item) => item.marker?.kind === "deferred" && item.marker.date === tomorrow);
}

/** The Not today line a loose-end decision writes to today's note; null for an item that can't be carried or lacks a reference. */
export function decisionEntry(row: PlanReviewItem, kind: DecisionKind, tomorrow: string, deferUntil?: string): NotTodayEntry | null {
  if (!row.reference || (kind === "carry" && row.state === "unavailable")) return null;
  const base = { reference: row.reference, label: row.title };
  if (kind === "drop") return { ...base, decision: { kind: "dropped" } };
  // Carrying to tomorrow is a deferral to tomorrow's date, which tomorrow's review and Plan Tomorrow start from.
  const date = kind === "carry" ? tomorrow : deferUntil;
  return date && date >= tomorrow ? { ...base, decision: { kind: "deferred", date } } : null;
}

/** One loose-end decision as the single item-level change it saves, with its own operation id. */
export function decisionChanges(entry: NotTodayEntry): { operationId: string; changes: PlanChanges } {
  return { operationId: newOperationId(), changes: { items: [], remove: [], notToday: [entry] } };
}

/**
 * Whether the day is closed: every ended meeting is reviewed or set aside and
 * no planned item is undecided. `null` for a count that isn't known yet, so
 * a day is never called closed on a guess.
 */
export function dayClosed(outstandingMeetings: number | null, undecidedItems: number | null): boolean {
  return outstandingMeetings === 0 && undecidedItems === 0;
}
