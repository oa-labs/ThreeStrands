import { useId, useLayoutEffect, useRef, useState, type KeyboardEvent } from "react";
import { ContextRow } from "./ContextSections";
import { formatClockTime } from "./agendaEvents";
import { readLocalStorage, useLocalStorageState } from "./useLocalStorageState";

export type DoneCategory = "tasks" | "emails" | "notes";
export type DoneFilter = "all" | DoneCategory;
export const DONE_FILTER_KEY = "threestrands.today.doneFilter";
/** More rows than this collapse to this many behind Show All. */
export const DONE_COLLAPSED_ROWS = 5;
const FILTERS: { id: DoneFilter; label: string }[] = [
  { id: "all", label: "All" }, { id: "tasks", label: "Tasks" }, { id: "emails", label: "Emails" }, { id: "notes", label: "Notes" },
];
const ERROR_LABELS: Record<string, string> = { tasks: "Tasks", noteTasks: "Note tasks", emails: "Sent mail", notes: "Notes" };

export type DoneEntry = {
  id: string;
  /** Every durable identity this row names, so a planned task and its completed task merge. */
  identities: string[];
  category: DoneCategory;
  title: string;
  /** When it was finished, sent or created (ISO); rows without one come last. */
  at: string | null;
  row: Omit<Parameters<typeof ContextRow>[0], "title" | "date" | "onPointerDown" | "actions" | "trailing">;
};

/** The first entry for each identity wins, so list the better-dated sources first. */
export function uniqueDoneEntries(entries: DoneEntry[]): DoneEntry[] {
  const seen = new Set<string>();
  return entries.filter((entry) => {
    const duplicate = entry.identities.some((identity) => seen.has(identity));
    for (const identity of entry.identities) seen.add(identity);
    return !duplicate;
  });
}

/** Newest first across every category; rows without a time keep their order after the timed ones. */
export function sortDoneEntries(entries: DoneEntry[]): DoneEntry[] {
  const time = (entry: DoneEntry) => { const value = entry.at ? Date.parse(entry.at) : NaN; return Number.isNaN(value) ? null : value; };
  return entries.map((entry, index) => ({ entry, index, value: time(entry) })).sort((left, right) =>
    left.value === right.value ? left.index - right.index : left.value === null ? 1 : right.value === null ? -1 : right.value - left.value).map(({ entry }) => entry);
}

/** "3 tasks · 2 emails · 1 note", leaving out what is zero. */
export function doneSummary(entries: readonly DoneEntry[]): string {
  const count = (category: DoneCategory) => entries.filter((entry) => entry.category === category).length;
  const part = (category: DoneCategory, one: string, many: string) => { const total = count(category); return total ? `${total} ${total === 1 ? one : many}` : null; };
  return [part("tasks", "task", "tasks"), part("emails", "email", "emails"), part("notes", "note", "notes")].filter(Boolean).join(" · ");
}

function initialFilter(): DoneFilter {
  return readLocalStorage<DoneFilter>(DONE_FILTER_KEY, (raw) => FILTERS.some((filter) => filter.id === raw) ? raw as DoneFilter : "all", "all");
}

/** What got finished on the day the schedule shows: tasks, mail the user sent, and notes they started. */
export function DoneCard({ title, today, dayLabel, entries, errors, loading }: {
  /** "Done Today", or "Done" for another day, as the schedule card is named. */
  title: string;
  today: boolean;
  /** The day, written out, when it is not today. */
  dayLabel?: string;
  entries: DoneEntry[];
  /** Why a source could not be read, by source: tasks, noteTasks, emails, notes. */
  errors: Partial<Record<"tasks" | "noteTasks" | "emails" | "notes", string | null | undefined>>;
  loading: boolean;
}) {
  const [filter, setFilter] = useLocalStorageState<DoneFilter>(DONE_FILTER_KEY, initialFilter);
  const [expanded, setExpanded] = useState(false);
  const id = useId();
  const tabs = useRef<HTMLDivElement>(null);
  const panel = useRef<HTMLDivElement>(null);
  const reopened = useRef<HTMLInputElement | null>(null);
  const sorted = sortDoneEntries(entries);
  const items = sorted.filter((entry) => filter === "all" || entry.category === filter);
  const collapsible = items.length > DONE_COLLAPSED_ROWS;
  const visible = collapsible && !expanded ? items.slice(0, DONE_COLLAPSED_ROWS) : items;
  const noun = sorted.length === 1 ? "item" : "items";
  const summary = doneSummary(sorted);
  const shownErrors = Object.entries(errors).filter(([source, message]) => message && (filter === "all"
    || (filter === "tasks" ? source === "tasks" || source === "noteTasks" : source === filter)));
  // Reopening a task takes its row out of this card; keep keyboard focus on the filters instead of losing it.
  useLayoutEffect(() => {
    const trigger = reopened.current;
    if (!trigger || trigger.isConnected) return;
    reopened.current = null;
    if (document.activeElement === document.body) tabs.current?.querySelector<HTMLButtonElement>('[aria-selected="true"]')?.focus();
  }, [entries]);
  const select = (next: DoneFilter, focus = false) => {
    setFilter(next); setExpanded(false);
    if (panel.current) panel.current.scrollTop = 0;
    if (focus) tabs.current?.querySelector<HTMLButtonElement>(`[data-filter="${next}"]`)?.focus();
  };
  const move = (event: KeyboardEvent) => {
    const at = FILTERS.findIndex((candidate) => candidate.id === filter);
    const next = event.key === "Home" ? FILTERS[0] : event.key === "End" ? FILTERS.at(-1)
      : event.key === "ArrowRight" ? FILTERS[(at + 1) % FILTERS.length]
        : event.key === "ArrowLeft" ? FILTERS[(at - 1 + FILTERS.length) % FILTERS.length] : null;
    if (!next) return;
    event.preventDefault(); event.stopPropagation(); select(next.id, true);
  };
  return <section aria-label={title} className="planning-inbox today-done">
    <header className="today-panel-heading">
      <h2>{title} <span className="context-count" aria-label={`${sorted.length} finished ${noun}`}>{sorted.length}</span>{dayLabel ? <small className="today-done-day">{dayLabel}</small> : null}</h2>
      {summary ? <span className="today-source-scope">{summary}</span> : null}
    </header>
    <div ref={tabs} className="planning-inbox-tabs" role="tablist" aria-label={`${title} Filters`} onKeyDown={move}>
      {FILTERS.map((choice) => <button key={choice.id} type="button" className="planning-inbox-tab" role="tab" id={`${id}-${choice.id}`} data-filter={choice.id}
        aria-selected={filter === choice.id} aria-controls={`${id}-items`} tabIndex={filter === choice.id ? 0 : -1} onClick={() => select(choice.id)}>{choice.label}</button>)}
    </div>
    <div ref={panel} className="today-done-items" role="tabpanel" tabIndex={0} id={`${id}-items`} aria-labelledby={`${id}-${filter}`} aria-busy={loading} onChangeCapture={(event) => {
      const target = event.target;
      if (target instanceof HTMLInputElement && target.type === "checkbox" && document.activeElement === target) reopened.current = target;
    }}>
      {shownErrors.map(([source, message]) => <p key={source} className="form-error" role="alert">{ERROR_LABELS[source]}: {message}</p>)}
      {!items.length ? <p className="context-status">{loading ? "Reading finished items…" : today ? "Nothing finished yet" : "Nothing recorded"}</p> : null}
      {visible.map((entry) => <ContextRow {...entry.row} key={entry.id} as="div" title={entry.title} wrapTitle date={entry.at && !Number.isNaN(Date.parse(entry.at)) ? formatClockTime(new Date(entry.at)) : undefined}
        className={[entry.row.className, "context-task context-task-done"].filter(Boolean).join(" ")} />)}
      {collapsible ? <button type="button" className="btn-link context-link-button" aria-expanded={expanded} onClick={() => setExpanded((value) => !value)}>{expanded ? "Show Fewer" : "Show All"}</button> : null}
    </div>
  </section>;
}
