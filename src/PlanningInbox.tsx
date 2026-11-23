import { CalendarClock, Inbox } from "lucide-react";
import { useId, useImperativeHandle, useLayoutEffect, useRef, useState, type KeyboardEvent, type ReactNode, type PointerEvent as ReactPointerEvent, type Ref } from "react";
import { ContextRow } from "./ContextSections";
import { MoreActionsMenu, type MoreAction } from "./MoreActionsMenu";
import { errorMessage } from "./errors";
import { ICON_SIZE } from "./iconSizes";
import { formatShortcut } from "./shortcutLabels";
import { readLocalStorage, useLocalStorageState } from "./useLocalStorageState";
import { useWorkspaceCommands } from "./workspaceCommands";
import type { PlanDragSource } from "./TodayPlan";

export type InboxCategory = "tasks" | "emails" | "contacts";
export type InboxFilter = "all" | "commitments" | InboxCategory;
export const INBOX_FILTER_KEY = "threestrands.today.inboxFilter";
export const INBOX_PAGE_SIZE = 8;
const FILTERS: { id: InboxFilter; label: string }[] = [
  { id: "all", label: "All" }, { id: "tasks", label: "Tasks" }, { id: "emails", label: "Emails" }, { id: "contacts", label: "Contacts" }, { id: "commitments", label: "Commitments" },
];

export type PlanningInboxHandle = {
  clearSelection(): void;
};

export type InboxEntry = {
  id: string;
  /** All durable identities this card names, so a planned task and its starred conversation merge. */
  identities: string[];
  category: InboxCategory;
  title: string;
  row: Omit<Parameters<typeof ContextRow>[0], "title" | "onPointerDown" | "actions" | "trailing" | "current" | "tabIndex" | "allowTabShortcut" | "rowRef">;
  source: PlanDragSource | null;
  schedule?(): Promise<unknown> | void;
  /** Opens the linked conversation, note, or contact for keyboard navigation (`o`). */
  openSource?(): void;
  /** Adds an unplanned candidate to today's plan (`a`). */
  addToPlan?(): Promise<unknown> | void;
  /** Removes a planned item from today's plan or dismisses a promise/commitment (`#`). */
  removeOrDismiss?(): Promise<unknown> | void;
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
export function readInboxFilter(): InboxFilter {
  const previous = readLocalStorage("threestrands.today.planSource", (raw) => raw, null);
  const fallback: InboxFilter = previous === "starred-mail" || previous === "promises" ? "emails"
    : previous === "keep-in-touch" ? "contacts"
      : previous && ["tasks", "also-due", "notes", "goals"].includes(previous) ? "tasks" : "all";
  return readLocalStorage(INBOX_FILTER_KEY, (raw) => FILTERS.some((filter) => filter.id === raw) ? raw as InboxFilter : fallback, fallback);
}

export function PlanningInbox({ ref, entries, commitments = [], onFilterChange, onSelectionChange, scope, onAddTask, status, unavailable = () => false, loading, hasMore, onLoadMore, drag }: {
  ref?: Ref<PlanningInboxHandle>;
  entries: InboxEntry[];
  commitments?: InboxEntry[];
  onFilterChange?(filter: InboxFilter): void;
  onSelectionChange?(hasSelection: boolean): void;
  scope?: string;
  /** Creates a task from a typed title. Present only when tasks can be created. Resolve to keep the input for the next task; reject to show the error. */
  onAddTask?(title: string): Promise<void> | void;
  status(filter: InboxFilter): ReactNode;
  unavailable?(filter: InboxFilter): boolean;
  loading(filter: InboxFilter): boolean;
  hasMore(filter: InboxFilter): boolean;
  onLoadMore(filter: InboxFilter): void;
  drag: {
    startDrag(source: PlanDragSource, event: ReactPointerEvent<HTMLElement>): void; clickSuppressed(): boolean;
    /** The row being dragged, shown dimmed. */
    draggingId: string | null;
    /** False while the schedule shows a day that can't take a booking, so rows show no grab cursor. */
    enabled: boolean;
  };
}) {
  const [filter, setFilter] = useLocalStorageState<InboxFilter>(INBOX_FILTER_KEY, readInboxFilter);
  const [limit, setLimit] = useState(INBOX_PAGE_SIZE);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const tabs = useRef<HTMLDivElement>(null);
  const id = useId();
  const panel = useRef<HTMLDivElement>(null);
  const rowElements = useRef(new Map<string, HTMLElement>());
  const lastSelected = useRef<{ id: string; index: number } | null>(null);
  const focusSelected = useRef(false);
  const pendingAddTaskFocus = useRef(false);
  const pending = useRef(new Set<string>());
  const [busyEntries, setBusyEntries] = useState(new Set<string>());
  const [actionError, setActionError] = useState<string | null>(null);
  const restore = useRef<{ id: string; index: number; trigger: HTMLButtonElement } | null>(null);
  const completionFocus = useRef<{ key: string; trigger: HTMLInputElement } | null>(null);
  const items = filter === "commitments" ? commitments : entries.filter((entry) => filter === "all" || entry.category === filter);
  const busy = loading(filter);
  const more = hasMore(filter);
  const singular = items.length === 1;
  const countLabel = filter === "commitments" ? (singular ? "commitment" : "commitments") : (singular ? "unscheduled item" : "unscheduled items");
  const count = items.length <= limit + 1 ? items.length : limit;
  const selectedIndex = selectedId === null ? -1 : items.findIndex((entry) => entry.id === selectedId);
  const selectedEntry = selectedIndex >= 0 ? items[selectedIndex] : null;
  if (selectedEntry) lastSelected.current = { id: selectedEntry.id, index: selectedIndex };
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
  const hasSelection = selectedEntry !== null;
  useLayoutEffect(() => {
    onSelectionChange?.(hasSelection);
  }, [hasSelection, onSelectionChange]);
  useLayoutEffect(() => {
    const request = completionFocus.current;
    if (!request || request.trigger.isConnected) return;
    completionFocus.current = null;
    if (document.activeElement !== document.body) return;
    const row = [...(panel.current?.querySelectorAll<HTMLElement>("[data-row-key]") ?? [])].find((element) => element.dataset.rowKey === request.key && !element.closest("[hidden]"));
    const target = row?.querySelector<HTMLInputElement>('input[type="checkbox"]')
      ?? tabs.current?.querySelector<HTMLButtonElement>('[aria-selected="true"]');
    target?.focus();
  }, [entries, filter]);
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
  useLayoutEffect(() => {
    if (selectedId === null || items.some((entry) => entry.id === selectedId)) return;
    const previous = lastSelected.current;
    if (!items.length || !previous) {
      lastSelected.current = null;
      setSelectedId(null);
      return;
    }
    const nextIndex = Math.min(previous.index, items.length - 1);
    const next = items[nextIndex];
    lastSelected.current = { id: next.id, index: nextIndex };
    if (document.activeElement === document.body) focusSelected.current = true;
    setSelectedId(next.id);
  }, [items, selectedId]);
  useLayoutEffect(() => {
    if (!focusSelected.current || !selectedId) return;
    const element = rowElements.current.get(selectedId);
    if (!element) return;
    focusSelected.current = false;
    element.focus();
    element.scrollIntoView?.({ block: "nearest" });
  }, [selectedId, count, items]);
  useLayoutEffect(() => {
    if (!pendingAddTaskFocus.current || !showAddTask) return;
    pendingAddTaskFocus.current = false;
    draftInput.current?.focus();
    draftInput.current?.scrollIntoView?.({ block: "nearest" });
  }, [showAddTask, filter]);
  const clearSelection = () => {
    const current = selectedId ? rowElements.current.get(selectedId) : null;
    lastSelected.current = null;
    focusSelected.current = false;
    setSelectedId(null);
    if (current && document.activeElement === current) current.blur();
  };
  useImperativeHandle(ref, () => ({ clearSelection }));
  const runEntryAction = async (entry: InboxEntry, action: () => Promise<unknown> | void) => {
    if (entry.busy || pending.current.has(entry.id)) return;
    pending.current.add(entry.id); setBusyEntries(new Set(pending.current)); setActionError(null);
    try { await action(); }
    catch (reason) { setActionError(errorMessage(reason)); }
    finally { pending.current.delete(entry.id); setBusyEntries(new Set(pending.current)); }
  };
  const schedule = async (entry: InboxEntry, index: number, trigger?: HTMLButtonElement) => {
    if (entry.busy || pending.current.has(entry.id)) return;
    if (trigger) restore.current = { id: entry.id, index, trigger };
    await runEntryAction(entry, () => entry.schedule?.());
  };
  const select = (next: InboxFilter, focus = false) => {
    const current = selectedId ? rowElements.current.get(selectedId) : null;
    lastSelected.current = null;
    focusSelected.current = false;
    setSelectedId(null);
    if (!focus && current && document.activeElement === current) current.blur();
    setFilter(next); setLimit(INBOX_PAGE_SIZE); onFilterChange?.(next);
    if (focus) tabs.current?.querySelector<HTMLButtonElement>(`[data-filter="${next}"]`)?.focus();
  };
  const cycleFilter = (direction: -1 | 1) => {
    const at = FILTERS.findIndex((candidate) => candidate.id === filter);
    const next = FILTERS[(at + direction + FILTERS.length) % FILTERS.length];
    select(next.id, Boolean(tabs.current?.contains(document.activeElement)));
  };
  const moveSelection = (direction: -1 | 1) => {
    if (!items.length) return;
    const nextIndex = selectedIndex < 0
      ? (direction > 0 ? 0 : Math.min(count, items.length) - 1)
      : Math.max(0, Math.min(items.length - 1, selectedIndex + direction));
    if (nextIndex >= count) setLimit((value) => Math.max(value + INBOX_PAGE_SIZE, nextIndex + 1));
    const next = items[nextIndex];
    if (!next) return;
    lastSelected.current = { id: next.id, index: nextIndex };
    if (next.id === selectedId) {
      const element = rowElements.current.get(next.id);
      element?.focus();
      element?.scrollIntoView?.({ block: "nearest" });
      return;
    }
    focusSelected.current = true;
    setSelectedId(next.id);
  };
  const focusAddTask = () => {
    if (!onAddTask) return;
    if (filter === "all" || filter === "tasks") {
      clearSelection();
      draftInput.current?.focus();
      draftInput.current?.scrollIntoView?.({ block: "nearest" });
    } else {
      pendingAddTaskFocus.current = true;
      select("tasks");
    }
  };
  const entryBusy = (entry: InboxEntry) => Boolean(entry.busy || busyEntries.has(entry.id));
  const canComplete = Boolean(selectedEntry && selectedEntry.row.control && !entryBusy(selectedEntry));
  const canSchedule = Boolean(selectedEntry && selectedEntry.schedule && !entryBusy(selectedEntry));
  const canAddToPlan = Boolean(selectedEntry && selectedEntry.addToPlan && !entryBusy(selectedEntry));
  const canRemoveOrDismiss = Boolean(selectedEntry && selectedEntry.removeOrDismiss && !entryBusy(selectedEntry));
  useWorkspaceCommands({
    "today.nextInboxTab": () => cycleFilter(1),
    "today.previousInboxTab": () => cycleFilter(-1),
    "today.nextItem": () => moveSelection(1),
    "today.previousItem": () => moveSelection(-1),
    ...(selectedEntry ? { "today.clearSelection": clearSelection } : {}),
    ...(selectedEntry?.row.onActivate ? { "today.openSelected": () => { if (!drag.clickSuppressed()) selectedEntry.row.onActivate?.(); } } : {}),
    ...(selectedEntry?.openSource ? { "today.openSource": () => selectedEntry.openSource?.() } : {}),
    ...(canComplete && selectedEntry ? {
      "today.completeSelected": () => {
        rowElements.current.get(selectedEntry.id)?.querySelector<HTMLInputElement>('input[type="checkbox"]:not(:disabled)')?.click();
      },
    } : {}),
    ...(canSchedule && selectedEntry ? { "today.scheduleSelected": () => void schedule(selectedEntry, selectedIndex) } : {}),
    ...(canAddToPlan && selectedEntry ? { "today.addToPlan": () => void runEntryAction(selectedEntry, () => selectedEntry.addToPlan?.()) } : {}),
    ...(canRemoveOrDismiss && selectedEntry ? { "today.removeOrDismiss": () => void runEntryAction(selectedEntry, () => selectedEntry.removeOrDismiss?.()) } : {}),
    ...(onAddTask ? { "today.addTask": focusAddTask } : {}),
  });
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
      <h2><Inbox size={ICON_SIZE.md} aria-hidden="true" />Planning Inbox <span className="context-count" aria-label={`${items.length}${more ? " or more" : ""} ${countLabel}`}>{items.length}{more ? "+" : ""}</span></h2>
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
      {!items.length && !unavailable(filter) && (busy || !more) ? <p className="context-status">{busy ? "Reading planning items…" : filter === "contacts" ? "You're up to date with all keep in touch contact goals" : filter === "commitments" ? "No commitments to review." : "Nothing to schedule here."}</p> : null}
      {items.slice(0, count).map((entry, index) => {
        const selected = selectedEntry?.id === entry.id;
        return <ContextRow {...entry.row} key={entry.id} as="article" title={entry.title} rowKey={rowKey(entry)} current={selected} tabIndex={-1} allowTabShortcut
          rowRef={(node) => { if (node) rowElements.current.set(entry.id, node); else rowElements.current.delete(entry.id); }}
          className={[entry.row.className, "today-inbox-card", selected ? "today-inbox-card-selected" : "", entry.source && drag.enabled ? "today-inbox-card-draggable" : "", drag.draggingId === entry.id ? "today-inbox-card-dragging" : ""].filter(Boolean).join(" ")}
          onActivate={entry.row.onActivate ? () => { if (!drag.clickSuppressed()) entry.row.onActivate?.(); } : undefined}
          onPointerDown={entry.source ? (event) => { if (drag.enabled && !entry.busy && !pending.current.has(entry.id) && entry.source) drag.startDrag(entry.source, event); } : undefined}
          actions={[
            ...(entry.schedule ? [<button key="schedule" type="button" className="btn-icon btn-icon-sm" aria-label={`Schedule: ${entry.title}`} aria-keyshortcuts="s" title={`Schedule the next free half hour (${formatShortcut("s")})`} disabled={entry.busy || busyEntries.has(entry.id)} onClick={(event) => void schedule(entry, index, event.currentTarget)}><CalendarClock size={ICON_SIZE.sm} /></button>] : []),
            ...(entry.actions.length ? [<MoreActionsMenu key="more" compact label={`Actions for ${entry.title}`} actions={entry.actions.map((action) => ({
              ...action, disabled: entry.busy || busyEntries.has(entry.id) || action.disabled,
              onSelect: () => {
                const trigger = document.activeElement;
                if (trigger instanceof HTMLButtonElement) restore.current = { id: entry.id, index, trigger };
                action.onSelect();
              },
            }))} />] : []),
          ]} />;
      })}
      {count < items.length || more ? <button type="button" className="btn-link context-link-button" disabled={busy} onClick={() => {
        if (count < items.length) setLimit((value) => value + INBOX_PAGE_SIZE);
        else onLoadMore(filter);
      }}>Show More</button> : null}
      {showAddTask ? <div className="today-add-task" data-shortcut-scope="modal">
        <input ref={draftInput} type="text" className="today-add-task-input" aria-label="Add a task" aria-keyshortcuts="d" title={`Add a task (${formatShortcut("d")})`} placeholder="Add a task…" value={draftTitle} readOnly={adding}
          aria-invalid={draftError ? true : undefined}
          onChange={(event) => { setDraftTitle(event.target.value); setDraftError(null); }}
          onKeyDown={(event) => {
            if (event.key === "Enter") { event.preventDefault(); void submitDraft(); }
            else if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); setDraftTitle(""); setDraftError(null); event.currentTarget.blur(); }
          }} />
        {draftError ? <p className="form-error" role="alert">{draftError}</p> : null}
      </div> : null}
    </div>
  </section>;
}
