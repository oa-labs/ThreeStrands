import { CalendarClock } from "lucide-react";
import { useId, useLayoutEffect, useRef, useState, type KeyboardEvent, type ReactNode, type PointerEvent as ReactPointerEvent } from "react";
import { ContextRow, ContextSection } from "./ContextSections";
import { MoreActionsMenu, type MoreAction } from "./MoreActionsMenu";
import { errorMessage } from "./errors";
import { ICON_SIZE } from "./iconSizes";
import { readLocalStorage, useLocalStorageState } from "./useLocalStorageState";
import type { PlanDragSource } from "./TodayPlan";

export type InboxCategory = "tasks" | "emails" | "contacts";
export type InboxFilter = "all" | InboxCategory;
export const INBOX_FILTER_KEY = "threestrands.today.inboxFilter";
export const INBOX_PAGE_SIZE = 8;
const FILTERS: { id: InboxFilter; label: string }[] = [
  { id: "all", label: "All" }, { id: "tasks", label: "Tasks" }, { id: "emails", label: "Emails" }, { id: "contacts", label: "Contacts" },
];

export type InboxEntry = {
  id: string;
  /** All durable identities this card names, so a planned task and its starred conversation merge. */
  identities: string[];
  category: InboxCategory;
  title: string;
  row: Omit<Parameters<typeof ContextRow>[0], "title" | "onPointerDown" | "actions" | "trailing">;
  source: PlanDragSource | null;
  schedule?(): Promise<unknown> | void;
  actions: MoreAction[];
  busy?: boolean;
};

/** Planned entries lead. Every source after them contributes only new identities. */
export function uniqueInboxEntries(entries: InboxEntry[]): InboxEntry[] {
  const seen = new Set<string>();
  return entries.filter((entry) => {
    const duplicate = entry.identities.some((identity) => seen.has(identity));
    for (const identity of entry.identities) seen.add(identity);
    return !duplicate;
  });
}

/** A mixed All list gives each category a turn while preserving its own ordering. */
export function interleaveInboxCategories(entries: InboxEntry[]): InboxEntry[] {
  const groups = (["tasks", "emails", "contacts"] as const).map((category) => entries.filter((entry) => entry.category === category));
  const result: InboxEntry[] = [];
  const length = Math.max(0, ...groups.map((group) => group.length));
  for (let index = 0; index < length; index++) for (const group of groups) if (group[index]) result.push(group[index]);
  return result;
}

/** Carry the previous source choice into the new, smaller set of filters. */
function initialFilter(): InboxFilter {
  const previous = readLocalStorage("threestrands.today.planSource", (raw) => raw, null);
  const fallback: InboxFilter = previous === "starred-mail" || previous === "promises" ? "emails"
    : previous === "keep-in-touch" ? "contacts"
      : previous && ["tasks", "also-due", "notes", "goals"].includes(previous) ? "tasks" : "all";
  return readLocalStorage(INBOX_FILTER_KEY, (raw) => FILTERS.some((filter) => filter.id === raw) ? raw as InboxFilter : fallback, fallback);
}

export function PlanningInbox({ entries, completed = [], completedError, scope, onAddTask, status, unavailable = () => false, loading, hasMore, onLoadMore, drag }: {
  entries: InboxEntry[];
  completed?: InboxEntry[];
  completedError?: string | null;
  scope?: string;
  /** Creates a task from a typed title. Present only when tasks can be created. Resolve to keep the input for the next task; reject to show the error. */
  onAddTask?(title: string): Promise<void> | void;
  status(filter: InboxFilter): ReactNode;
  unavailable?(filter: InboxFilter): boolean;
  loading(filter: InboxFilter): boolean;
  hasMore(filter: InboxFilter): boolean;
  onLoadMore(filter: InboxFilter): void;
  drag: { startDrag(source: PlanDragSource, event: ReactPointerEvent<HTMLElement>): void; clickSuppressed(): boolean };
}) {
  const [filter, setFilter] = useLocalStorageState<InboxFilter>(INBOX_FILTER_KEY, initialFilter);
  const [limit, setLimit] = useState(INBOX_PAGE_SIZE);
  const tabs = useRef<HTMLDivElement>(null);
  const id = useId();
  const panel = useRef<HTMLDivElement>(null);
  const pending = useRef(new Set<string>());
  const [busyEntries, setBusyEntries] = useState(new Set<string>());
  const [actionError, setActionError] = useState<string | null>(null);
  const restore = useRef<{ id: string; index: number; trigger: HTMLButtonElement } | null>(null);
  const completionFocus = useRef<{ key: string; trigger: HTMLInputElement } | null>(null);
  const items = entries.filter((entry) => filter === "all" || entry.category === filter);
  const busy = loading(filter);
  const more = hasMore(filter);
  const noun = items.length === 1 ? "item" : "items";
  const count = items.length <= limit + 1 ? items.length : limit;
  const completedItems = completed.filter((entry) => filter === "all" || filter === "tasks" || entry.category === filter);
  const showCompleted = filter !== "contacts";
  const [draftTitle, setDraftTitle] = useState("");
  const [draftError, setDraftError] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);
  const draftInput = useRef<HTMLInputElement>(null);
  const showAddTask = Boolean(onAddTask) && (filter === "all" || filter === "tasks");
  const submitDraft = async () => {
    const title = draftTitle.trim();
    if (!title || adding || !onAddTask) return;
    setAdding(true); setDraftError(null);
    try { await onAddTask(title); setDraftTitle(""); }
    catch (reason) { setDraftError(errorMessage(reason)); }
    finally { setAdding(false); draftInput.current?.focus(); }
  };
  const rowKey = (entry: InboxEntry) => entry.identities.find((identity) => identity.startsWith("task:")) ?? entry.id;
  useLayoutEffect(() => {
    const request = completionFocus.current;
    if (!request || request.trigger.isConnected) return;
    completionFocus.current = null;
    if (document.activeElement !== document.body) return;
    const row = [...(panel.current?.querySelectorAll<HTMLElement>("[data-row-key]") ?? [])].find((element) => element.dataset.rowKey === request.key && !element.closest("[hidden]"));
    const target = row?.querySelector<HTMLInputElement>('input[type="checkbox"]')
      ?? panel.current?.querySelector<HTMLButtonElement>(".today-completed-card .context-section-toggle")
      ?? tabs.current?.querySelector<HTMLButtonElement>('[aria-selected="true"]');
    target?.focus();
  }, [entries, completed, filter]);
  useLayoutEffect(() => {
    const request = restore.current;
    if (!request || items.some((entry) => entry.id === request.id)) return;
    restore.current = null;
    if (request.trigger.isConnected || document.activeElement !== document.body) return;
    const rows = panel.current?.querySelectorAll<HTMLElement>("article");
    const row = rows?.[Math.min(request.index, rows.length - 1)];
    const target = row?.querySelector<HTMLButtonElement>(".context-row-main")
      ?? tabs.current?.querySelector<HTMLButtonElement>('[aria-selected="true"]');
    target?.focus();
  }, [items]);
  const schedule = async (entry: InboxEntry, index: number, trigger: HTMLButtonElement) => {
    if (entry.busy || pending.current.has(entry.id)) return;
    restore.current = { id: entry.id, index, trigger };
    pending.current.add(entry.id); setBusyEntries(new Set(pending.current)); setActionError(null);
    try { await entry.schedule?.(); }
    catch (reason) { setActionError(errorMessage(reason)); }
    finally { pending.current.delete(entry.id); setBusyEntries(new Set(pending.current)); }
  };
  const select = (next: InboxFilter, focus = false) => {
    setFilter(next); setLimit(INBOX_PAGE_SIZE);
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
  return <section aria-label="Planning Inbox" className="planning-inbox">
    <header className="today-panel-heading">
      <h2>Planning Inbox <span className="context-count" aria-label={`${items.length}${more ? " or more" : ""} unscheduled ${noun}`}>{items.length}{more ? "+" : ""}</span></h2>
      {scope && filter !== "contacts" ? <span className="today-source-scope">Mail · {scope}</span> : null}
    </header>
    <div ref={tabs} className="planning-inbox-tabs" role="tablist" aria-label="Planning Inbox Filters" onKeyDown={move}>
      {FILTERS.map((choice) => <button key={choice.id} type="button" className="planning-inbox-tab" role="tab" id={`${id}-${choice.id}`} data-filter={choice.id}
        aria-selected={filter === choice.id} aria-controls={`${id}-items`} tabIndex={filter === choice.id ? 0 : -1} onClick={() => select(choice.id)}>{choice.label}</button>)}
    </div>
    <div ref={panel} role="tabpanel" id={`${id}-items`} aria-labelledby={`${id}-${filter}`} aria-busy={busy} onChangeCapture={(event) => {
      const target = event.target;
      if (target instanceof HTMLInputElement && target.type === "checkbox" && document.activeElement === target) {
        const key = target.closest<HTMLElement>("[data-row-key]")?.dataset.rowKey;
        if (key) completionFocus.current = { key, trigger: target };
      }
    }}>
      {status(filter)}
      {actionError ? <p className="form-error" role="alert">{actionError}</p> : null}
      {!items.length && !unavailable(filter) && (busy || !more) ? <p className="context-status">{busy ? "Reading planning items…" : filter === "contacts" ? "You're up to date with all keep in touch contact goals" : "Nothing to schedule here."}</p> : null}
      {items.slice(0, count).map((entry, index) => <ContextRow {...entry.row} key={entry.id} as="article" title={entry.title} rowKey={rowKey(entry)}
        className={[entry.row.className, "today-inbox-card"].filter(Boolean).join(" ")}
        onActivate={entry.row.onActivate ? () => { if (!drag.clickSuppressed()) entry.row.onActivate?.(); } : undefined}
        onPointerDown={entry.source ? (event) => { if (!entry.busy && !pending.current.has(entry.id) && entry.source) drag.startDrag(entry.source, event); } : undefined}
        actions={[
          ...(entry.schedule ? [<button key="schedule" type="button" className="btn-icon btn-icon-sm" aria-label={`Schedule: ${entry.title}`} title="Schedule the next free half hour" disabled={entry.busy || busyEntries.has(entry.id)} onClick={(event) => void schedule(entry, index, event.currentTarget)}><CalendarClock size={ICON_SIZE.sm} /></button>] : []),
          ...(entry.actions.length ? [<MoreActionsMenu key="more" compact label={`Actions for ${entry.title}`} actions={entry.actions.map((action) => ({ ...action, disabled: entry.busy || busyEntries.has(entry.id) || action.disabled }))} />] : []),
        ]} />)}
      {count < items.length || more ? <button type="button" className="btn-link context-link-button" disabled={busy} onClick={() => {
        if (count < items.length) setLimit((value) => value + INBOX_PAGE_SIZE);
        else onLoadMore(filter);
      }}>Show More</button> : null}
      {showAddTask ? <div className="today-add-task">
        <input ref={draftInput} type="text" className="today-add-task-input" aria-label="Add a task" placeholder="Add a task…" value={draftTitle} readOnly={adding}
          aria-invalid={draftError ? true : undefined}
          onChange={(event) => { setDraftTitle(event.target.value); setDraftError(null); }}
          onKeyDown={(event) => {
            if (event.key === "Enter") { event.preventDefault(); void submitDraft(); }
            else if (event.key === "Escape" && draftTitle) { event.preventDefault(); event.stopPropagation(); setDraftTitle(""); setDraftError(null); }
          }} />
        {draftError ? <p className="form-error" role="alert">{draftError}</p> : null}
      </div> : null}
      {showCompleted && (completedItems.length || (filter !== "emails" && completedError)) ? <ContextSection id="today-completed" title="Completed" className="today-completed-card" count={completedItems.length}
        status={filter !== "emails" && completedError ? <p className="form-error" role="alert">Completed note tasks: {completedError}</p> : undefined}
        rows={completedItems.map((entry) => <ContextRow {...entry.row} key={entry.id} as="div" title={entry.title} rowKey={rowKey(entry)} className="context-task context-task-done" />)} /> : null}
    </div>
  </section>;
}
