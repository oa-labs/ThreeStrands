import { planItemKey, planLine, type PlanAddition } from "./dailyPlan";
import type { DailyPlan, DailyPlanItem, NotTodayEntry, PlanChanges, PlanReviewItem } from "./domain";
import type { NoteReference } from "./noteEntities";
import type { BlockTarget } from "./todayBooking";

/**
 * Plan Today's routine as plain data: the steps, the user's choices, the
 * changes a save sends, and the bounded local record that restores an
 * unfinished routine after a restart. Nothing here writes a note.
 */
export const PLAN_STEPS = ["review", "choose", "meetings", "fit", "save"] as const;
export type PlanStep = (typeof PLAN_STEPS)[number];
export const PLAN_STEP_LABELS: Record<PlanStep, string> = { review: "Review", choose: "Choose", meetings: "Meetings", fit: "Fit", save: "Save" };
/** Above this many items, Choose notes that most people finish three to five. */
export const PLAN_ITEMS_NOTE = 5;
/** From this many carries, a Didn't get to row hints at deferring or dropping (as `PLAN_CARRY_HINT` natively). */
export const PLAN_CARRY_HINT = 3;
export const ROUTINE_RECOVERY_KEY = "threestrands.planRoutine";
/** Unfinished routines kept on this device, newest first. */
export const MAX_RECOVERED_ROUTINES = 2;
/** Days an unfinished routine is kept. */
export const ROUTINE_RECOVERY_DAYS = 7;

/** One item in the routine's ordered list. */
export type RoutineItem = {
  /** Unique in the list: the plan key, or `new:<n>` for a typed task. */
  id: string;
  /** The plan key it names, or null for a typed task. */
  key: string | null;
  title: string;
  /** Already a line in the plan. */
  existing: boolean;
  /** What a save writes for a new item. */
  addition: PlanAddition | null;
  /** The earlier plan date a carried item came from. */
  carried: string | null;
  /** What Book books for it; null for free text. */
  block: BlockTarget | null;
  /** Its source is no longer on this device; it isn't saved. */
  missing?: boolean;
};

export type ReviewDecision = { kind: "carry" } | { kind: "defer"; date: string } | { kind: "drop" };
/** A decision about a Didn't get to row, with what writing it needs. */
export type DecisionRecord = ReviewDecision & { key: string; reference: NoteReference; label: string; plannedOn: string };
/** A block booked in Fit: a calendar write already made. */
export type RoutineBooking = { title: string; start: string; eventId: string };

export type RoutineState = {
  version: 1;
  date: string;
  /** The note version the routine started from, which the save checks. */
  base: string | null;
  /** Reopened with Edit Plan: the plan already had items. */
  edit: boolean;
  /** One id per routine, so a save retried after a lost response isn't repeated. */
  operationId: string;
  step: PlanStep;
  items: RoutineItem[];
  /** The plan's own items, in their order when the routine started. */
  initial: string[];
  /** Existing items taken off the plan. */
  removed: RoutineItem[];
  decisions: Record<string, DecisionRecord>;
  bookings: RoutineBooking[];
  /** Typed tasks so far, for their ids. */
  typed: number;
  savedAt: string;
};

const newOperationId = () => (globalThis.crypto?.randomUUID?.() ?? `plan-${Date.now()}-${Math.random().toString(36).slice(2)}`);

/** What booking a plan item books: its task, or a check-in with no task. */
export function blockForItem(item: Pick<DailyPlanItem, "kind" | "title" | "taskId" | "accountId">): BlockTarget | null {
  if (item.kind === "text") return null;
  return item.kind === "contact"
    ? { title: `Check in with ${item.title}`, sourceTaskId: null, accountId: null }
    : { title: item.title, sourceTaskId: item.taskId, accountId: item.accountId };
}

/** A fresh routine for `date`, starting from the plan's own items in order. */
export function startRoutine(date: string, plan: DailyPlan | null, now = new Date()): RoutineState {
  const items: RoutineItem[] = (plan?.items ?? []).map((item) => ({
    id: item.key, key: item.key, title: item.title, existing: true, addition: null, carried: null,
    block: item.state === "unavailable" ? null : blockForItem(item), missing: false,
  }));
  return {
    version: 1, date, base: plan?.revision ?? null, edit: items.length > 0, operationId: newOperationId(), step: "review",
    items, initial: items.map((item) => item.id), removed: [], decisions: {}, bookings: [], typed: 0, savedAt: now.toISOString(),
  };
}

const touch = (state: RoutineState, patch: Partial<RoutineState>): RoutineState => ({ ...state, ...patch, savedAt: new Date().toISOString() });

/** Adds an item at the end, unless one with the same key is already chosen; a removed plan item comes back. */
export function addItem(state: RoutineState, item: Omit<RoutineItem, "id" | "existing"> & { id?: string }): RoutineState {
  if (item.key && state.items.some((candidate) => candidate.key === item.key)) return state;
  const returning = item.key ? state.removed.find((candidate) => candidate.key === item.key) : undefined;
  if (returning) return touch(state, { items: [...state.items, returning], removed: state.removed.filter((candidate) => candidate !== returning) });
  const typed = item.key ? state.typed : state.typed + 1;
  const id = item.id ?? item.key ?? `new:${typed}`;
  return touch(state, { items: [...state.items, { ...item, id, existing: false }], typed });
}

/** A typed, plan-only round task. */
export function addNewTask(state: RoutineState, title: string): RoutineState {
  const clean = title.split(/\s+/).filter(Boolean).join(" ");
  if (!clean) return state;
  return addItem(state, { key: null, title: clean, addition: { kind: "newTask", title: clean }, carried: null, block: { title: clean, sourceTaskId: null, accountId: null } });
}

/** Takes an item off: a plan item's line is removed on save (not dropped); a carried one undoes its carry. */
export function removeItem(state: RoutineState, id: string): RoutineState {
  const item = state.items.find((candidate) => candidate.id === id);
  if (!item) return state;
  const decisions = { ...state.decisions };
  if (item.key && decisions[item.key]?.kind === "carry") delete decisions[item.key];
  return touch(state, {
    items: state.items.filter((candidate) => candidate !== item),
    removed: item.existing ? [...state.removed, item] : state.removed,
    decisions,
  });
}

/** Moves an item up (-1) or down (+1) within the list. */
export function moveItem(state: RoutineState, id: string, step: number): RoutineState {
  const at = state.items.findIndex((item) => item.id === id);
  const to = at + step;
  if (at < 0 || to < 0 || to >= state.items.length) return state;
  const items = [...state.items];
  [items[at], items[to]] = [items[to], items[at]];
  return touch(state, { items });
}

/** Carry, Defer…, or Drop a Didn't get to row, or `null` to leave it undecided. Carrying adds it to the chosen items. */
export function decide(state: RoutineState, row: PlanReviewItem, decision: ReviewDecision | null): RoutineState {
  if (!row.reference) return state;
  const decisions = { ...state.decisions };
  let next = state;
  const carriedHere = state.items.find((item) => item.key === row.key && !item.existing);
  if (carriedHere && decision?.kind !== "carry") next = { ...next, items: next.items.filter((item) => item !== carriedHere) };
  if (decision === null) delete decisions[row.key];
  else {
    if (decision.kind === "carry" && row.state === "unavailable") return state;
    decisions[row.key] = { ...decision, key: row.key, reference: row.reference, label: row.title, plannedOn: row.plannedOn };
  }
  next = touch(next, { decisions });
  if (decision?.kind === "carry") {
    next = addItem(next, { key: row.key, title: row.title, addition: { kind: "link", reference: row.reference, label: row.title }, carried: row.plannedOn, block: blockForItem(row) });
  }
  return next;
}

/** Carry All: every undecided, available row is carried. */
export function carryAll(state: RoutineState, rows: readonly PlanReviewItem[]): RoutineState {
  return rows.filter((row) => !state.decisions[row.key] && row.state !== "unavailable").reduce((next, row) => decide(next, row, { kind: "carry" }), state);
}

const notTodayEntries = (state: RoutineState): NotTodayEntry[] => Object.values(state.decisions).flatMap((decision): NotTodayEntry[] =>
  decision.kind === "defer" ? [{ reference: decision.reference, label: decision.label, decision: { kind: "deferred" as const, date: decision.date } }]
    : decision.kind === "drop" ? [{ reference: decision.reference, label: decision.label, decision: { kind: "dropped" as const } }] : []);

/** What Save Plan sends: the items in order, the plan items taken off, and the Not today decisions. */
export function planChanges(state: RoutineState): PlanChanges {
  return {
    items: state.items.filter((item) => !item.missing).flatMap((item): PlanChanges["items"] => item.existing ? [{ kind: "existing" as const, key: item.key! }]
      : item.addition ? [{ kind: "add" as const, addition: item.addition, carried: item.carried }] : []),
    remove: state.removed.map((item) => item.key!),
    notToday: notTodayEntries(state),
  };
}

/** Whether saving would change the note. */
export function isDirty(state: RoutineState): boolean {
  const changes = planChanges(state);
  const order = state.items.filter((item) => item.existing && !item.missing).map((item) => item.id);
  const kept = state.initial.filter((id) => order.includes(id));
  return changes.items.some((entry) => entry.kind === "add") || changes.remove.length > 0 || changes.notToday.length > 0 || order.join("\n") !== kept.join("\n");
}

/** Whether the routine holds any choice worth asking about before closing. */
export function hasChoices(state: RoutineState): boolean {
  return isDirty(state) || state.bookings.length > 0;
}

/** Exactly the Markdown lines a save adds: new plan items, then Not today lines. */
export function previewLines(state: RoutineState): { plan: string[]; notToday: string[] } {
  return {
    plan: state.items.filter((item) => !item.existing && !item.missing && item.addition).map((item) => planLine(item.addition!, item.carried ? { kind: "carried", date: item.carried } : null)),
    notToday: notTodayEntries(state).map((entry) => planLine({ kind: "link", reference: entry.reference, label: entry.label }, entry.decision)),
  };
}

const weekday = (date: string) => new Intl.DateTimeFormat(undefined, { weekday: "long", month: "long", day: "numeric" }).format(new Date(`${date}T12:00:00`));

/** The plan as plain text, for Copy Plan. */
export function planText(state: RoutineState): string {
  const lines = [`Plan for ${weekday(state.date)}`, "", ...state.items.filter((item) => !item.missing).map((item, at) => `${at + 1}. ${item.title}`)];
  const later = Object.values(state.decisions).filter((decision) => decision.kind !== "carry");
  if (later.length) {
    lines.push("", "Not today");
    for (const decision of later) lines.push(`- ${decision.label} (${decision.kind === "defer" ? `deferred to ${weekday(decision.date)}` : "dropped"})`);
  }
  return `${lines.join("\n")}\n`;
}

/** Marks chosen items whose sources are gone, such as a task deleted while the routine waited. */
export function markMissing(state: RoutineState, present: (item: RoutineItem) => boolean): RoutineState {
  let changed = false;
  const items = state.items.map((item) => {
    const missing = !present(item);
    if (missing === Boolean(item.missing)) return item;
    changed = true;
    return { ...item, missing };
  });
  return changed ? { ...state, items } : state;
}

/** Re-bases a restored routine on the plan as it is now: plan items no longer there are marked missing. */
export function rebase(state: RoutineState, plan: DailyPlan | null): RoutineState {
  const keys = new Set((plan?.items ?? []).map((item) => item.key));
  return {
    ...markMissing(state, (item) => item.existing ? keys.has(item.key!) : !item.missing),
    removed: state.removed.filter((item) => keys.has(item.key!)),
    base: plan?.revision ?? null,
    initial: state.initial.filter((id) => keys.has(id)),
  };
}

/** The key a candidate adds under, for marking it chosen. */
export function additionKey(addition: PlanAddition): string | null {
  return addition.kind === "link" ? planItemKey(addition.reference) : null;
}

type Stored = { routines: RoutineState[] };
const isState = (value: unknown): value is RoutineState => {
  const state = value as RoutineState;
  return Boolean(state) && state.version === 1 && typeof state.date === "string" && Array.isArray(state.items) && Array.isArray(state.removed)
    && typeof state.operationId === "string" && (PLAN_STEPS as readonly string[]).includes(state.step) && typeof state.savedAt === "string";
};

function readAll(now: Date): RoutineState[] {
  try {
    const raw = window.localStorage.getItem(ROUTINE_RECOVERY_KEY);
    const parsed = raw ? (JSON.parse(raw) as Stored) : null;
    const cutoff = now.getTime() - ROUTINE_RECOVERY_DAYS * 86_400_000;
    return (Array.isArray(parsed?.routines) ? parsed.routines : []).filter(isState).filter((state) => Date.parse(state.savedAt) >= cutoff);
  } catch {
    return [];
  }
}

function writeAll(routines: RoutineState[]) {
  try {
    if (routines.length) window.localStorage.setItem(ROUTINE_RECOVERY_KEY, JSON.stringify({ routines } satisfies Stored));
    else window.localStorage.removeItem(ROUTINE_RECOVERY_KEY);
  } catch {
    // Recovery is a convenience; the routine still works without it.
  }
}

/** The unfinished routine for `date`, if this device kept one. */
export function readRecovery(date: string, now = new Date()): RoutineState | null {
  return readAll(now).find((state) => state.date === date) ?? null;
}

/** Keeps `state` for recovery: one per date, the newest `MAX_RECOVERED_ROUTINES`, none older than `ROUTINE_RECOVERY_DAYS`. */
export function writeRecovery(state: RoutineState, now = new Date()) {
  const others = readAll(now).filter((candidate) => candidate.date !== state.date);
  writeAll([state, ...others].sort((a, b) => Date.parse(b.savedAt) - Date.parse(a.savedAt)).slice(0, MAX_RECOVERED_ROUTINES));
}

export function discardRecovery(date: string, now = new Date()) {
  writeAll(readAll(now).filter((state) => state.date !== date));
}
