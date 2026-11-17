import type { ReactNode } from "react";
import { formatClockTime } from "./agendaEvents";
import type { DailyPlanItem, PlanCompletedTask, PlanReviewItem, ScheduleEvent } from "./domain";
import { PLAN_CARRY_HINT } from "./planRoutine";
import { carriedLabel } from "./TodayPlan";

/** A date as "Mon, Nov 23", for a plan's day or a deferral. */
export const shortPlanDate = (date: string) => new Intl.DateTimeFormat(undefined, { weekday: "short", month: "short", day: "numeric" }).format(new Date(`${date}T12:00:00`));

/** What a review's Done column lists: the plan's finished items, meetings attended, and tasks done outside the plan. */
export type ReviewDoneProps = {
  done: readonly DailyPlanItem[];
  meetings: readonly ScheduleEvent[];
  alsoDone: readonly PlanCompletedTask[];
  /** Said when all three are empty. */
  empty: string;
  /** Turns a row into a button that opens its source, where the surface can leave itself. */
  onOpenItem?(item: DailyPlanItem): void;
  onOpenTask?(task: PlanCompletedTask): void;
  onOpenMeeting?(event: ScheduleEvent): void;
};

const opens = (label: ReactNode, open: (() => void) | undefined) => open ? <button type="button" className="btn-link" onClick={open}>{label}</button> : label;

/**
 * The Done list shared by Plan Today's Review step and Today's evening view:
 * read-only rows, no totals. Rows open their source only where the caller
 * passes a handler.
 */
export function ReviewDone({ done, meetings, alsoDone, empty, onOpenItem, onOpenTask, onOpenMeeting }: ReviewDoneProps) {
  return <>
    <ul className="plan-review-list">
      {done.map((item) => <li key={item.key}>{opens(item.title, onOpenItem && (() => onOpenItem(item)))}</li>)}
      {meetings.map((event) => <li key={`${event.id}:${event.start}`}>{opens(event.title, onOpenMeeting && (() => onOpenMeeting(event)))} <small>{formatClockTime(new Date(event.start))}</small></li>)}
    </ul>
    {alsoDone.length ? <>
      <h4>Also done</h4>
      <ul className="plan-review-list">{alsoDone.map((task) => <li key={task.key}>{opens(task.title, onOpenTask && (() => onOpenTask(task)))}</li>)}</ul>
    </> : null}
    {!done.length && !alsoDone.length && !meetings.length ? <p className="context-status">{empty}</p> : null}
  </>;
}

export type DecisionKind = "carry" | "defer" | "drop";

/**
 * One unfinished item with its Carry, Defer…, and Drop buttons, shared by
 * Plan Today's Review step (where a choice is pressed, and the Defer date
 * opens beneath the row) and Today's evening view (where a choice is saved at
 * once and the Defer date is asked in a small dialog).
 */
export function ReviewDecisionRow({ item, decision = null, deferredTo = null, busy = false, carryLabel = "Carry", deferExpanded, onDecide, children }: {
  item: PlanReviewItem;
  /** The choice already made, shown pressed. */
  decision?: DecisionKind | null;
  deferredTo?: string | null;
  busy?: boolean;
  carryLabel?: string;
  /** Marks Defer… as opening something beneath the row. */
  deferExpanded?: boolean;
  onDecide(kind: DecisionKind): void;
  /** The Defer date's control, in Plan Today's Review. */
  children?: ReactNode;
}) {
  const carries = item.carryCount ?? 0;
  const unavailable = item.state === "unavailable";
  const detail = [unavailable ? "Unavailable on this device" : null, carriedLabel(carries)].filter(Boolean).join(" · ");
  return <li className="plan-review-row">
    <div className="plan-review-text">
      <strong>{item.title}</strong>
      {detail ? <small>{detail}</small> : null}
      {carries >= PLAN_CARRY_HINT ? <small className="plan-review-hint">Carried {carries} times. Defer or drop it?</small> : null}
      {decision === "defer" && deferredTo ? <small>Deferred to {shortPlanDate(deferredTo)}</small> : null}
    </div>
    <div className="plan-review-actions" role="group" aria-label={`Decide: ${item.title}`}>
      <button type="button" className="btn btn-sm" aria-pressed={decision === "carry"} disabled={unavailable || busy} onClick={() => onDecide("carry")}>{carryLabel}</button>
      <button type="button" className="btn btn-sm" aria-pressed={decision === "defer"} aria-expanded={deferExpanded} disabled={busy} onClick={() => onDecide("defer")}>Defer…</button>
      <button type="button" className="btn btn-sm" aria-pressed={decision === "drop"} disabled={busy} onClick={() => onDecide("drop")}>Drop</button>
    </div>
    {children}
  </li>;
}
