import { CalendarClock, CircleMinus, CirclePlus, ContactRound, NotebookPen } from "lucide-react";
import { useRef, type KeyboardEvent as ReactKeyboardEvent, type PointerEvent as ReactPointerEvent, type ReactNode } from "react";
import { HoverTooltip } from "./AppChrome";
import { formatClockTime } from "./agendaEvents";
import { ContextRow, ContextSection } from "./ContextSections";
import { MoreActionsMenu } from "./MoreActionsMenu";
import type { PlanAddition } from "./dailyPlan";
import type { DailyPlan, DailyPlanItem, ScheduleEvent } from "./domain";
import { ICON_SIZE } from "./iconSizes";
import { bookedFor, type BlockTarget } from "./todayBooking";
import { bookedSource, sourceTaskId } from "./taskBooking";

/** One key for a task however its booked block names it, so a card and its block can find each other. */
export function linkKey(taskId: string | null | undefined): string | null {
  const source = bookedSource(taskId);
  return source ? sourceTaskId(source) : null;
}

/** "New Task for Goal" as a tooltip reads: "New task for goal". */
const sentence = (label: string) => label.charAt(0) + label.slice(1).toLowerCase();

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
  /** The add button's name when adding first creates something, such as "New Task for Goal". */
  addLabel?: string;
  /** How its row reads; Add to Today's Plan and dragging are added by the switcher. */
  row: Omit<RowProps, "title" | "onPointerDown" | "trailing">;
};

export type PlanSourceGroup = { id: string; title: string; candidates: PlanCandidate[]; limit?: number; closing?: ReactNode;
  /** Under the rows, such as Show More for the next page. */
  footer?: ReactNode };

/**
 * A tab of the source switcher. Today registers Also Due, Tasks, You Said You
 * Would, and Keep in Touch; further sources (Starred Mail, Goals, Notes) are
 * added by appending another entry built the same way.
 */
export type PlanSource = {
  id: string;
  label: string;
  groups: PlanSourceGroup[];
  /** Shown when no group has a card, such as "Nothing overdue or due today." */
  empty: string;
  /** Hides candidates already in the plan instead of marking them. */
  hidePlanned?: boolean;
  /** Loading or a read failure, shown above the groups; no fallback is shown meanwhile. */
  status?: ReactNode;
  /** Kept behind the More Sources menu until chosen, so the tab row stays one line. */
  overflow?: boolean;
  /** The mail account its cards follow, shown beside the tabs while it is chosen. */
  scope?: string;
  /** Always last in its panel, such as Open Tasks. */
  footer?: ReactNode;
};

/** Cards a fallback shows from the next source when the chosen one is empty. */
export const SOURCE_FALLBACK_CARDS = 3;

/** What dragging a plan card or a source card carries onto the day view. */
export type PlanDragSource = { id: string; title: string } & ({ kind: "item"; item: DailyPlanItem } | { kind: "candidate"; candidate: PlanCandidate });

type DragHooks = {
  startDrag(source: PlanDragSource, event: ReactPointerEvent<HTMLElement>): void;
  /** True for the click a drag ends with, which must not activate the card. */
  clickSuppressed(): boolean;
};

/** Items whose state can finish: everything but free text. */
export function trackedItems(plan: DailyPlan | null): DailyPlanItem[] {
  return (plan?.items ?? []).filter((item) => item.kind !== "text");
}

export const isFinished = (item: DailyPlanItem) => item.state === "done" || item.state === "cancelled";
const isOpen = (item: DailyPlanItem) => item.state === "open" || item.state === "inProgress";

/** What a block booked for the item is for: its task, or a check-in with no task. */
export function itemBlock(item: DailyPlanItem): BlockTarget {
  return item.kind === "contact"
    ? { title: `Check in with ${item.title}`, sourceTaskId: null, accountId: null }
    : { title: item.title, sourceTaskId: item.taskId, accountId: item.accountId };
}

/** "3 of 5 planned done", or null without tracked items. */
export function planProgress(plan: DailyPlan | null): string | null {
  const tracked = trackedItems(plan);
  return tracked.length ? `${tracked.filter(isFinished).length} of ${tracked.length} planned done` : null;
}

function itemSource(item: DailyPlanItem): string | null {
  if (item.state === "unavailable") return "Unavailable on this device";
  if (item.kind === "noteTask" || item.kind === "newTask") return "Note";
  if (item.kind === "contact") return "Keep in Touch";
  return item.accountId;
}

/**
 * Today's plan: one card per item in order. The first open item is the
 * day's focus. Tasks can be checked off; every tracked card offers Book and
 * Remove from Today's Plan and can be dragged onto the day view.
 */
/** "Carried 3 times", from the plan's history; a fact, never a reason to reorder. */
export function carriedLabel(count: number): string | null {
  return count >= 1 ? `Carried ${count} ${count === 1 ? "time" : "times"}` : null;
}

export function PlanCards({ plan, error, events, now, onToggle, onActivate, onBook, onRemove, drag, linked, onLink }: {
  plan: DailyPlan | null;
  error: string | null;
  /** Today's events, for each task's booked time. */
  events: readonly ScheduleEvent[];
  now: Date;
  onToggle(item: DailyPlanItem): void;
  onActivate(item: DailyPlanItem): void;
  onBook(item: DailyPlanItem): void;
  onRemove(item: DailyPlanItem): void;
  drag: DragHooks;
  /** The task whose card and booked block are highlighted together. */
  linked?: string | null;
  /** Hovering or focusing a card with a task, and leaving it. */
  onLink?(taskId: string | null): void;
}) {
  const items = plan?.items ?? [];
  const focus = items.find(isOpen);
  const rows = items.map((item) => {
    const booked = bookedFor(item.taskId, [...events], now);
    const detail = [itemSource(item), item.goal?.title, booked ? formatClockTime(new Date(booked.start)) : null, carriedLabel(item.carryCount ?? 0)].filter(Boolean).join(" · ");
    const checkable = (item.kind === "task" || item.kind === "noteTask" || item.kind === "newTask") && item.taskId && item.state !== "unavailable" && item.state !== "cancelled";
    const done = isFinished(item);
    const isLinked = Boolean(linked && item.taskId && linkKey(item.taskId) === linked);
    const classes = ["context-task", "today-plan-card", done ? "context-task-done" : "", item === focus ? "today-plan-focus" : "", isLinked ? "today-plan-linked" : ""].filter(Boolean).join(" ");
    const usable = item.state !== "unavailable" && item.kind !== "text";
    const hover = item.taskId && onLink ? {
      onMouseEnter: () => onLink(linkKey(item.taskId!)), onMouseLeave: () => onLink(null),
      onFocus: () => onLink(linkKey(item.taskId!)), onBlur: () => onLink(null),
    } : {};
    return <div key={item.key} className="today-plan-card-link" {...hover}><ContextRow
      as="article"
      className={classes}
      rowKey={`plan:${item.key}`}
      control={checkable ? <HoverTooltip label={done ? "Mark not done" : "Mark done"}>
        <input type="checkbox" className="context-task-checkbox" checked={done}
          aria-label={done ? `Mark ${item.title} not done` : `Mark ${item.title} done`} onChange={() => onToggle(item)} />
      </HoverTooltip> : undefined}
      icon={item.kind === "contact" ? <ContactRound size={ICON_SIZE.sm} /> : item.kind === "newTask" || item.kind === "noteTask" ? <NotebookPen size={ICON_SIZE.sm} /> : undefined}
      title={item.title}
      wrapTitle
      detail={detail || undefined}
      onActivate={item.kind === "text" ? undefined : () => { if (!drag.clickSuppressed()) onActivate(item); }}
      onPointerDown={usable && !done ? (event) => drag.startDrag({ id: `plan:${item.key}`, title: item.title, kind: "item", item }, event) : undefined}
      actions={[
        ...(usable && !done ? [<button key="book" type="button" className="btn-icon btn-icon-sm" aria-label={`Book: ${item.title}`} title="Book the next free half hour" onClick={() => onBook(item)}>
          <CalendarClock size={ICON_SIZE.sm} />
        </button>] : []),
        <button key="remove" type="button" className="btn-icon btn-icon-sm" aria-label={`Remove from Today's Plan: ${item.title}`} title="Remove from today's plan" onClick={() => onRemove(item)}>
          <CircleMinus size={ICON_SIZE.sm} />
        </button>,
      ]}
    /></div>;
  });
  return <ContextSection
    id="today-plan"
    title="Today's plan"
    count={items.length || undefined}
    limit={items.length}
    rows={rows}
    footer={error ? <p className="form-error" role="alert">{error}</p>
      : plan && items.length === 0 ? <p className="context-status">Nothing is planned yet. Choose Add to Today's Plan on a card below, or drag one onto the day.</p> : undefined}
  />;
}

/**
 * One source at a time, chosen from a tab list (arrow keys, Home, and End
 * move between tabs). Cards already in the plan are marked "In plan" rather
 * than offered again.
 */
export function PlanSourceSwitcher({ sources, selected, onSelect, planned, onAdd, drag }: {
  sources: PlanSource[];
  selected: string;
  onSelect(id: string): void;
  /** Keys of the targets already in the plan. */
  planned: ReadonlySet<string>;
  onAdd(candidate: PlanCandidate): void;
  drag: DragHooks;
}) {
  const tabs = useRef<HTMLDivElement>(null);
  const current = sources.find((source) => source.id === selected) ?? sources[0];
  if (!current) return null;
  const choose = (target: PlanSource) => {
    onSelect(target.id);
    // An overflow source joins the row once chosen, so focus it after it renders.
    requestAnimationFrame(() => tabs.current?.querySelector<HTMLButtonElement>(`[data-source="${target.id}"]`)?.focus());
  };
  // Arrow keys, Home, and End move through every source, those behind More Sources included.
  const move = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    const at = sources.indexOf(current);
    const target = event.key === "ArrowRight" ? sources[(at + 1) % sources.length]
      : event.key === "ArrowLeft" ? sources[(at - 1 + sources.length) % sources.length]
        : event.key === "Home" ? sources[0] : event.key === "End" ? sources.at(-1) : undefined;
    if (!target) return;
    event.preventDefault();
    event.stopPropagation();
    tabs.current?.querySelector<HTMLButtonElement>(`[data-source="${target.id}"]`)?.focus();
    choose(target);
  };
  const inPlan = (candidate: PlanCandidate) => Boolean(candidate.inPlan) || (candidate.planKey !== null && planned.has(candidate.planKey));
  const groupsOf = (source: PlanSource) => source.groups.map((group) => ({ ...group, candidates: source.hidePlanned ? group.candidates.filter((candidate) => !inPlan(candidate)) : group.candidates }))
    .filter((group) => group.candidates.length > 0 || group.closing);
  const groups = groupsOf(current);
  // An empty source still shows, but the first view is never two empty states: the next source
  // with cards (in tab order) lends its first few beneath, with a way to switch to it.
  const fallback = groups.length || current.status ? null : (() => {
    const at = sources.indexOf(current);
    for (let step = 1; step < sources.length; step++) {
      const source = sources[(at + step) % sources.length];
      const lent = groupsOf(source).filter((group) => group.candidates.length > 0);
      if (!source.status && lent.length) return { source, groups: lent };
    }
    return null;
  })();
  const row = (candidate: PlanCandidate) => {
    const planned = inPlan(candidate);
    const activate = candidate.row.onActivate;
    return <ContextRow
      {...candidate.row}
      key={candidate.id}
      title={candidate.title}
      onActivate={activate ? () => { if (!drag.clickSuppressed()) activate(); } : undefined}
      onPointerDown={planned ? undefined : (event) => drag.startDrag({ id: candidate.id, title: candidate.title, kind: "candidate", candidate }, event)}
      trailing={planned ? <span className="today-in-plan">In plan</span> : undefined}
      actions={[
        ...(planned ? [] : [<button key="add" type="button" className="btn-icon btn-icon-sm" aria-label={`${candidate.addLabel ?? "Add to Today's Plan"}: ${candidate.title}`} title={candidate.addLabel ? sentence(candidate.addLabel) : "Add to today's plan"} onClick={() => onAdd(candidate)}>
          <CirclePlus size={ICON_SIZE.sm} />
        </button>]),
        ...(candidate.row.actions ?? []),
      ]}
    />;
  };
  const shown = sources.filter((source) => !source.overflow || source === current);
  const more = sources.filter((source) => source.overflow && source !== current);
  return <div className="today-sources">
    <div className="today-source-bar">
      <div className="mailbox-tab-list today-source-tabs" role="tablist" aria-label="Plan Sources" ref={tabs} onKeyDown={move}>
        {shown.map((source) => <button key={source.id} type="button" className={`mailbox-tab${source === current ? " active" : ""}`} role="tab" id={`today-source-${source.id}`}
          data-source={source.id} aria-controls="today-source-panel" aria-selected={source === current} tabIndex={source === current ? 0 : -1}
          onClick={() => onSelect(source.id)}>{source.label}</button>)}
      </div>
      {more.length ? <MoreActionsMenu label="More Sources" compact actions={more.map((source) => ({ label: source.label, onSelect: () => choose(source) }))} /> : null}
      {current.scope ? <span className="today-source-scope" title="These cards follow the mail account in view">{current.scope}</span> : null}
    </div>
    <div id="today-source-panel" role="tabpanel" aria-labelledby={`today-source-${current.id}`} className="today-source-panel">
      {current.status}
      {groups.length ? groups.map((group) => <ContextSection key={group.id} id={group.id} title={group.title} count={group.candidates.length}
        limit={group.limit} rows={group.candidates.map(row)} closing={group.closing} footer={group.footer} />)
        : current.status ? null : <p className="context-status">{current.empty}</p>}
      {fallback ? <div className="today-source-fallback">
        <p className="context-status today-source-lead">Meanwhile, from <button type="button" className="btn-link" onClick={() => choose(fallback.source)}>{fallback.source.label}</button>:</p>
        {fallback.groups.map((group) => <ContextSection key={group.id} id={group.id} title={group.title} count={group.candidates.length}
          limit={SOURCE_FALLBACK_CARDS} rows={group.candidates.slice(0, SOURCE_FALLBACK_CARDS).map(row)}
          footer={group.candidates.length > SOURCE_FALLBACK_CARDS
            ? <button type="button" className="btn-link context-link-button" onClick={() => choose(fallback.source)}>Show all {group.candidates.length} in {fallback.source.label}</button> : undefined} />)}
      </div> : null}
      {current.footer}
    </div>
  </div>;
}
