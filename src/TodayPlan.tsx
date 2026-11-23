import { ContextRow } from "./ContextSections";
import type { PlanAddition } from "./dailyPlan";
import type { DailyPlanItem } from "./domain";
import { type BlockTarget } from "./todayBooking";
import { bookedSource, sourceTaskId } from "./taskBooking";

/** One key for a task however its booked block names it, so a card and its block can find each other. */
export function linkKey(taskId: string | null | undefined): string | null {
  const source = bookedSource(taskId);
  return source ? sourceTaskId(source) : null;
}

type RowProps = Parameters<typeof ContextRow>[0];

/**
 * One card a source offers for the plan. A source registers its cards with
 * what adding writes; `resolve` may first create what the plan links to,
 * such as a promise's follow-up task, and returns null when it made nothing.
 */
export type PlanCandidate = {
  /** Unique among the cards on screen; also the drag source's id. */
  id: string;
  title: string;
  /** The target it names, keyed as plan items are (`DailyPlanItem.key`), so a planned one is marked. */
  planKey: string | null;
  resolve(): Promise<{ addition: PlanAddition; block: BlockTarget; title: string } | null>;
  /** Already in the plan by the source's own reckoning, such as a starred conversation whose task is planned. */
  inPlan?: boolean;
  /** Canonical task identity, when the source already has a task. */
  taskId?: string | null;
  /** The add button's name when adding first creates something, such as "New Task for Goal". */
  addLabel?: string;
  /** Opens the conversation, note, or contact this item came from, when one exists. */
  openSource?(): void;
  /** How its row reads; Add to Today's Plan and dragging are added by the switcher. */
  row: Omit<RowProps, "title" | "onPointerDown" | "trailing">;
};

/** What dragging a plan card or a source card carries onto the day view. */
export type PlanDragSource = { id: string; title: string } & ({ kind: "item"; item: DailyPlanItem } | { kind: "candidate"; candidate: PlanCandidate });

export const isFinished = (item: DailyPlanItem) => item.state === "done" || item.state === "cancelled";

/** What a block booked for the item is for: its task, or a check-in with no task. */
export function itemBlock(item: DailyPlanItem): BlockTarget {
  return item.kind === "contact"
    ? { title: `Check in with ${item.title}`, sourceTaskId: null, accountId: null, sourcePlanKey: item.key }
    : { title: item.title, sourceTaskId: item.taskId, accountId: item.accountId };
}

/** "Carried 3 times", from the plan's history; a fact, never a reason to reorder. */
export function carriedLabel(count: number): string | null {
  return count >= 1 ? `Carried ${count} ${count === 1 ? "time" : "times"}` : null;
}
