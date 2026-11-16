import type { ContactProfile, DailyPlan, DailyPlanItem, Goal, OpenPromise, PlanCandidate, PlanCandidateReason, PlanCompletedTask, PlanHistory, PlanReview, PlanReviewItem, PlanSourcePage, ScheduleEvent, Thread, ThreadTask } from "../domain";
import { insertPlanLines, parsePlan, planLine, removePlanLine, type PlanLine, type PlanSection } from "../dailyPlan";
import { normalizeContactAddress, noteReferenceKey, type NoteReference } from "../noteEntities";
import { periodFor } from "../goals";
import { compareTasksForDisplay, isActiveTaskStatus } from "../taskViews";
import type { MailClient } from "./client";

/** As natively (`MAX_PLAN_ADDITIONS`). */
export const MAX_PLAN_ADDITIONS = 50;
/** As natively: plan days a carry count and planned days look back over, including the last. */
export const PLAN_HISTORY_DAYS = 90;
/** As natively: days before the plan date the review looks for earlier plans. */
export const PLAN_LOOKBACK_DAYS = 7;
/** As natively: candidates in one page of a plan source. */
export const PLAN_SOURCE_PAGE = 50;
/** As natively: most plan item keys one history request names. */
export const MAX_PLAN_HISTORY_TARGETS = 200;
/** As natively: most unplanned completed tasks the review lists. */
export const MAX_ALSO_DONE = 50;
/** Carries from which a review row hints at deferring or dropping. */
export const PLAN_CARRY_HINT = 3;

type Sources = {
  tasks(): ThreadTask[];
  goals(): Goal[];
  contacts(): ContactProfile[] | Promise<ContactProfile[]>;
  notes: Pick<MailClient, "getDailyNote" | "saveDailyNote" | "getNote" | "listNoteTasks">;
  threads?(): Thread[];
  events?(): ScheduleEvent[];
  promises?(accountId?: string): Promise<OpenPromise[]>;
  now?(): Date;
};

/** `day` moved by `count` calendar days. */
export function shiftDay(day: string, count: number): string {
  const at = new Date(`${day}T12:00:00Z`);
  at.setUTCDate(at.getUTCDate() + count);
  return at.toISOString().slice(0, 10);
}
const localDay = (at: Date) => `${at.getFullYear()}-${String(at.getMonth() + 1).padStart(2, "0")}-${String(at.getDate()).padStart(2, "0")}`;
const daysBetween = (from: string, to: string) => Math.round((Date.parse(`${to}T12:00:00Z`) - Date.parse(`${from}T12:00:00Z`)) / 86_400_000);
const isDay = (value: string) => /^\d{4}-\d{2}-\d{2}$/.test(value) && !Number.isNaN(Date.parse(`${value}T00:00:00Z`));
const taskRef = (id: string): NoteReference => ({ version: 1, kind: "task", id });

type DayPlan = { date: string; noteId: string; section: PlanSection };
const tracked = (line: PlanLine) => line.kind !== "text";
const mentions = (plan: DayPlan, key: string) => [...plan.section.items, ...plan.section.notToday].some((line) => tracked(line) && keyOf(line) === key);

const keyOf = (line: PlanLine) => (line.reference ? noteReferenceKey(line.reference) : `line:${line.start}`);
const taskState = (status: ThreadTask["status"]): DailyPlanItem["state"] => status === "in_progress" ? "inProgress" : status === "completed" ? "done" : status === "cancelled" ? "cancelled" : "open";

/** The day's plan for the browser demo, following the native store's rules. */
export function createDemoPlans(sources: Sources): Pick<MailClient, "dailyPlan" | "addToDailyPlan" | "removeFromDailyPlan" | "planHistory" | "planReview" | "planSources"> {
  const now = () => sources.now?.() ?? new Date();
  /** The plans of `from..=to`, oldest first. */
  const plansBetween = async (from: string, to: string): Promise<DayPlan[]> => {
    const plans: DayPlan[] = [];
    for (let date = from; date <= to; date = shiftDay(date, 1)) {
      const note = await sources.notes.getDailyNote(date);
      const section = note && !note.trashed ? parsePlan(`daily:${date}`, note.body) : null;
      if (section) plans.push({ date, noteId: `daily:${date}`, section });
    }
    return plans;
  };
  const history = async (targets: string[], from: string, to: string): Promise<Map<string, PlanHistory>> => {
    const out = new Map(targets.map((target) => [target, { target, carryCount: 0, plannedDays: [] as string[] }]));
    if (!targets.length) return out;
    for (const plan of await plansBetween(from, to)) {
      const seen = new Map<string, boolean>();
      for (const line of plan.section.items.filter(tracked)) {
        const key = keyOf(line);
        if (out.has(key)) seen.set(key, (seen.get(key) ?? false) || line.marker?.kind === "carried");
      }
      for (const [key, carried] of seen) {
        const entry = out.get(key)!;
        entry.plannedDays.push(plan.date);
        if (carried) entry.carryCount += 1;
      }
    }
    return out;
  };
  const fillCarryCounts = async (date: string, items: DailyPlanItem[]) => {
    const counted = items.filter((item) => item.kind !== "text");
    const found = await history([...new Set(counted.map((item) => item.key))], shiftDay(date, -(PLAN_HISTORY_DAYS - 1)), date);
    for (const item of counted) item.carryCount = found.get(item.key)?.carryCount ?? 0;
  };
  const resolve = async (date: string, noteId: string, line: PlanLine): Promise<DailyPlanItem> => {
    const item: DailyPlanItem = { key: keyOf(line), kind: line.kind, reference: line.reference, title: line.label, state: "text", marker: line.marker, taskId: null, accountId: null, goal: null, contactId: null, carryCount: 0, start: line.start, end: line.end };
    const reference = line.reference;
    if (!reference) return item;
    if (line.kind === "task") {
      const task = sources.tasks().find((candidate) => candidate.id === reference.id);
      if (!task) return { ...item, state: "unavailable" };
      const goal = task.goalId ? sources.goals().find((candidate) => candidate.id === task.goalId) : undefined;
      return { ...item, title: task.title, state: taskState(task.status), taskId: task.id, accountId: task.accountId, goal: goal ? { id: goal.id, title: goal.title } : null };
    }
    if (line.kind === "noteTask") {
      const task = (await sources.notes.listNoteTasks()).find((candidate) => candidate.noteId === reference.note && candidate.markerId === reference.id);
      return task ? { ...item, title: task.title, state: task.checked ? "done" : "open", taskId: task.id } : { ...item, state: "unavailable" };
    }
    if (line.kind === "newTask") return { ...item, state: line.checked ? "done" : "open", taskId: line.markerId ? `note:${noteId}:${line.markerId}` : null };
    if (line.kind === "contact") {
      const contacts = await sources.contacts();
      const contact = contacts.find((candidate) => candidate.id === reference.contact) ?? contacts.find((candidate) => candidate.addresses.some((address) => address.toLowerCase() === reference.id));
      const touched = contact?.keepInTouch.lastTouchAt?.slice(0, 10);
      return { ...item, state: touched && touched >= date ? "done" : "open", contactId: contact && !contact.id.startsWith("derived:") ? contact.id : null };
    }
    return item;
  };
  const current = async (date: string, expected: string | null) => {
    const note = await sources.notes.getDailyNote(date);
    if ((note?.updatedAt ?? null) !== expected || note?.trashed) throw new Error("That day's note changed. Open it before changing the plan.");
    return note;
  };
  const save = async (date: string, expected: string | null, body: string) => {
    const saved = await sources.notes.saveDailyNote(date, body, expected);
    if (!saved.saved || !saved.note) throw new Error("That day's note changed. Open it before changing the plan.");
    return saved.note;
  };
  return {
    async dailyPlan(date): Promise<DailyPlan> {
      const noteId = `daily:${date}`;
      const note = await sources.notes.getDailyNote(date);
      const plan: DailyPlan = { date, noteId, revision: null, items: [], notToday: [] };
      if (!note || note.trashed) return plan;
      const section = parsePlan(noteId, note.body);
      const items = section ? await Promise.all(section.items.map((line) => resolve(date, noteId, line))) : [];
      const notToday = section ? await Promise.all(section.notToday.map((line) => resolve(date, noteId, line))) : [];
      await fillCarryCounts(date, [...items, ...notToday]);
      return { ...plan, revision: note.updatedAt, items, notToday };
    },
    async planHistory(targets, from, to) {
      if (targets.length > MAX_PLAN_HISTORY_TARGETS) throw new Error(`Ask for at most ${MAX_PLAN_HISTORY_TARGETS} plan items' history at once`);
      if (!isDay(from) || !isDay(to)) throw new Error("Invalid date");
      const span = daysBetween(from, to);
      if (span < 0 || span >= PLAN_HISTORY_DAYS) throw new Error(`Plan history covers 1 to ${PLAN_HISTORY_DAYS} days`);
      const unique = [...new Set(targets)];
      const found = await history(unique, from, to);
      return unique.map((target) => found.get(target)!);
    },
    async planReview(date): Promise<PlanReview> {
      if (!isDay(date)) throw new Error("Invalid date");
      const windowStart = shiftDay(date, -PLAN_LOOKBACK_DAYS);
      const plans = await plansBetween(windowStart, date);
      const settled = (key: string, plan: DayPlan) => plan.section.notToday.some((line) => tracked(line) && keyOf(line) === key)
        || plans.some((later) => later.date > plan.date && mentions(later, key));
      const earlier = plans.filter((plan) => plan.date < date && plan.section.items.length > 0).reverse();
      const reviewed = earlier[0] ?? null;
      const review: PlanReview = { date, reviewed: reviewed?.date ?? null, done: [], alsoDone: [], meetings: null, didntGetTo: [] };
      const unfinished = (item: DailyPlanItem) => item.state === "open" || item.state === "inProgress" || item.state === "unavailable";
      const listed = new Set<string>();
      for (const plan of earlier) {
        for (const line of plan.section.items.filter(tracked)) {
          const item = await resolve(plan.date, plan.noteId, line);
          if (plan === reviewed && item.state === "done") { review.done.push(item); continue; }
          if (!unfinished(item) || settled(item.key, plan) || listed.has(item.key)) continue;
          listed.add(item.key);
          review.didntGetTo.push({ ...item, plannedOn: plan.date });
        }
      }
      const due: [string, PlanReviewItem][] = [];
      const deferring = await plansBetween(shiftDay(date, -(PLAN_HISTORY_DAYS - 1)), shiftDay(date, -1));
      for (const plan of deferring) {
        for (const line of plan.section.notToday.filter(tracked)) {
          if (line.marker?.kind !== "deferred" || line.marker.date > date) continue;
          const key = keyOf(line);
          if (listed.has(key) || deferring.concat(plans).some((later) => later.date > plan.date && later.date <= date && mentions(later, key))) continue;
          const item = await resolve(plan.date, plan.noteId, line);
          if (!unfinished(item)) continue;
          listed.add(key);
          due.push([line.marker.date, { ...item, plannedOn: plan.date }]);
        }
      }
      due.sort((a, b) => a[0].localeCompare(b[0]) || a[1].plannedOn.localeCompare(b[1].plannedOn));
      review.didntGetTo.push(...due.map(([, item]) => item));
      await fillCarryCounts(date, [...review.done, ...review.didntGetTo]);
      if (reviewed) {
        const planned = new Set(reviewed.section.items.map(keyOf));
        const onDay = (at: string | null | undefined) => !!at && localDay(new Date(at)) === reviewed.date;
        const alsoDone: PlanCompletedTask[] = [
          ...sources.tasks().filter((task) => task.status === "completed" && onDay(task.completedAt))
            .map((task) => ({ key: noteReferenceKey(taskRef(task.id)), taskId: task.id, title: task.title, accountId: task.accountId, noteId: null, noteTitle: null, completedAt: task.completedAt! })),
          ...(await sources.notes.listNoteTasks()).filter((task) => task.checked && onDay(task.completedAt))
            .map((task) => ({ key: noteReferenceKey({ ...taskRef(task.markerId), note: task.noteId }), taskId: task.id, title: task.title, accountId: null, noteId: task.noteId, noteTitle: task.noteTitle, completedAt: task.completedAt! })),
        ];
        review.alsoDone = alsoDone.filter((task) => !planned.has(task.key))
          .sort((a, b) => Date.parse(a.completedAt) - Date.parse(b.completedAt) || a.taskId.localeCompare(b.taskId)).slice(0, MAX_ALSO_DONE);
        if (sources.events) {
          const start = new Date(`${reviewed.date}T00:00:00`).getTime(), end = new Date(`${shiftDay(reviewed.date, 1)}T00:00:00`).getTime();
          const day = sources.events().filter((event) => Date.parse(event.start) < end && Date.parse(event.end) > start);
          const seen = new Set<string>();
          review.meetings = day.length ? day.filter((event) => !event.allDay && event.responseStatus !== "declined" && event.eventType !== "workingLocation" && Date.parse(event.end) <= now().getTime())
            .sort((a, b) => Date.parse(a.start) - Date.parse(b.start))
            .filter((event) => { const identity = `${event.uid ?? event.id}\n${event.start}`; if (seen.has(identity)) return false; seen.add(identity); return true; }) : null;
        }
      }
      return review;
    },
    async planSources(date, source, accountId = null, cursor = null): Promise<PlanSourcePage> {
      if (!isDay(date)) throw new Error("Invalid date");
      const offset = cursor === null ? 0 : Number(cursor);
      if (!Number.isInteger(offset) || offset < 0) throw new Error("Invalid page cursor");
      const at = now();
      const inScope = (account: string) => !accountId || account === accountId;
      const goals = sources.goals();
      const base = (key: string, title: string, reason: PlanCandidateReason, action: PlanCandidate["action"]): PlanCandidate => ({ key, title, reason, detail: null, accountId: null, due: null, taskId: null, threadId: null, contactId: null, goal: null, plannedDays: [], openTasks: null, planned: false, action });
      const link = (reference: NoteReference, label: string): PlanCandidate["action"] => ({ kind: "link", addition: { kind: "link", reference, label } });
      const forTask = (task: ThreadTask, reason: PlanCandidateReason): PlanCandidate => {
        const goal = task.goalId ? goals.find((candidate) => candidate.id === task.goalId) : undefined;
        return { ...base(noteReferenceKey(taskRef(task.id)), task.title, reason, link(taskRef(task.id), task.title)), accountId: task.accountId, due: task.dueValue ?? null, taskId: task.id, threadId: task.threadId, goal: goal ? { id: goal.id, title: goal.title } : null };
      };
      const dueReason = (task: ThreadTask): PlanCandidateReason | null => {
        if (!task.dueValue || task.dueKind === "none") return null;
        const day = task.dueKind === "date" ? task.dueValue : localDay(new Date(task.dueValue));
        const passed = task.dueKind === "datetime" && Date.parse(task.dueValue) < at.getTime();
        return day < date || passed ? "overdue" : day === date ? "dueToday" : null;
      };
      const open = sources.tasks().filter((task) => isActiveTaskStatus(task.status) && task.source !== "note" && inScope(task.accountId)).sort(compareTasksForDisplay);
      const conversation = (thread: Thread): NoteReference => ({ version: 1, kind: "conversation", id: thread.providerThreadId, account: thread.accountId });
      const promises = async () => (await sources.promises?.(accountId ?? undefined) ?? []).map((promise) => ({
        ...base(noteReferenceKey(conversation(promise.thread)), promise.excerpt, "promise", { kind: "makeTask", threadId: promise.thread.id, messageId: promise.messageId }),
        detail: promise.recipients.join(", "), accountId: promise.thread.accountId, threadId: promise.thread.id,
      }));
      const checkIns = async () => (await sources.contacts()).filter((profile) => !profile.id.startsWith("derived:") && profile.keepInTouchDueAt && profile.addresses.length && localDay(new Date(profile.keepInTouchDueAt)) <= date)
        .sort((a, b) => Date.parse(a.keepInTouchDueAt!) - Date.parse(b.keepInTouchDueAt!))
        .map((profile) => {
          const reference: NoteReference = { version: 1, kind: "contact", id: normalizeContactAddress(profile.addresses[0]), contact: profile.id };
          const title = profile.displayName?.trim() || profile.addresses[0];
          return { ...base(noteReferenceKey(reference), title, "keepInTouch", link(reference, title)), due: profile.keepInTouchDueAt, contactId: profile.id };
        });
      const rank: PlanCandidateReason[] = ["overdue", "dueToday", "inProgress", "waiting"];
      let all: PlanCandidate[];
      if (source === "suggested") {
        const pressing = open.flatMap((task) => {
          const due = dueReason(task);
          const reason = task.kind === "waiting_for" && due ? "waiting" : due ?? (task.status === "in_progress" ? "inProgress" : null);
          return reason ? [forTask(task, reason)] : [];
        }).sort((a, b) => rank.indexOf(a.reason) - rank.indexOf(b.reason));
        all = [...pressing, ...await promises(), ...await checkIns()];
      } else if (source === "tasks") {
        all = open.map((task) => forTask(task, dueReason(task) ?? (task.status === "in_progress" ? "inProgress" : "open")));
      } else if (source === "starredMail") {
        all = (sources.threads?.() ?? []).filter((thread) => thread.starred && !thread.trashed && !thread.labels.includes("SPAM") && inScope(thread.accountId))
          .sort((a, b) => Date.parse(b.lastReceivedAt) - Date.parse(a.lastReceivedAt))
          .map((thread) => {
            const task = open.find((candidate) => candidate.threadId === thread.id);
            const candidate = task ? { ...forTask(task, "starred"), detail: thread.subject }
              : { ...base(noteReferenceKey(conversation(thread)), thread.subject, "starred", { kind: "taskForm", threadId: thread.id, accountId: thread.accountId }), detail: thread.participants[0] ?? null };
            return { ...candidate, accountId: thread.accountId, threadId: thread.id };
          });
      } else if (source === "promises") {
        all = await promises();
      } else if (source === "keepInTouch") {
        all = await checkIns();
      } else if (source === "goals") {
        const order = ["quarter", "half", "year"];
        const current = goals.filter((goal) => goal.status === "active" && inScope(goal.accountId) && goal.period === periodFor(goal.horizon, new Date(`${date}T12:00:00`)))
          .sort((a, b) => order.indexOf(a.horizon) - order.indexOf(b.horizon) || a.createdAt.localeCompare(b.createdAt));
        all = [];
        const starts = new Map<string, string>();
        for (const goal of current) {
          const tasks = sources.tasks().filter((task) => task.goalId === goal.id && isActiveTaskStatus(task.status)).sort(compareTasksForDisplay);
          all.push({ ...base(`goal:${goal.id}`, goal.title, "goal", { kind: "newTaskForGoal", goalId: goal.id, accountId: goal.accountId }), detail: goal.period, accountId: goal.accountId, goal: { id: goal.id, title: goal.title }, openTasks: tasks.length });
          const month = goal.horizon === "year" ? 1 : goal.horizon === "half" ? (Number(date.slice(5, 7)) <= 6 ? 1 : 7) : Math.floor((Number(date.slice(5, 7)) - 1) / 3) * 3 + 1;
          for (const task of tasks) {
            const candidate = { ...forTask(task, "goalTask"), goal: { id: goal.id, title: goal.title } };
            starts.set(candidate.key, `${date.slice(0, 4)}-${String(month).padStart(2, "0")}-01`);
            all.push(candidate);
          }
        }
        const found = await history([...starts.keys()], shiftDay(date, -(PLAN_HISTORY_DAYS - 1)), date);
        for (const candidate of all) if (starts.has(candidate.key)) candidate.plannedDays = (found.get(candidate.key)?.plannedDays ?? []).filter((day) => day >= starts.get(candidate.key)!);
      } else {
        const own = `daily:${date}`;
        const notes = (await sources.notes.listNoteTasks()).filter((task) => !task.checked && task.noteId !== own);
        const order = [...new Set(notes.map((task) => task.noteId))];
        all = notes.sort((a, b) => b.updatedAt.localeCompare(a.updatedAt) || order.indexOf(a.noteId) - order.indexOf(b.noteId))
          .map((task) => {
            const reference: NoteReference = { ...taskRef(task.markerId), note: task.noteId };
            return { ...base(noteReferenceKey(reference), task.title, "noteTask", link(reference, task.title)), detail: task.noteTitle, taskId: task.id };
          });
      }
      const note = await sources.notes.getDailyNote(date);
      const planned = new Set((note && !note.trashed ? parsePlan(`daily:${date}`, note.body)?.items ?? [] : []).map(keyOf));
      const candidates = all.slice(offset, offset + PLAN_SOURCE_PAGE).map((candidate) => ({ ...candidate, planned: planned.has(candidate.key) }));
      return { candidates, next: all.length > offset + PLAN_SOURCE_PAGE ? String(offset + candidates.length) : null };
    },
    async addToDailyPlan(date, expected, additions) {
      if (!additions.length || additions.length > MAX_PLAN_ADDITIONS) throw new Error(`Add between 1 and ${MAX_PLAN_ADDITIONS} plan items at once`);
      const noteId = `daily:${date}`;
      const note = await current(date, expected);
      const previousBody = note?.body ?? "";
      const present = (parsePlan(noteId, previousBody)?.items ?? []).map(keyOf);
      const lines: string[] = [], changed: string[] = [], already: string[] = [];
      for (const addition of additions) {
        if (addition.kind === "link") {
          const reference = addition.reference;
          if (reference.kind === "task" && !reference.note && !sources.tasks().some((task) => task.id === reference.id)) throw new Error("That item isn't on this device");
          if (reference.kind === "task" && reference.note) {
            const owner = await sources.notes.getNote(reference.note);
            if (!owner || owner.trashed) throw new Error("That item isn't on this device");
            if (!owner.body.includes(`<!-- threestrands-task:${reference.id} -->`)) throw new Error("Open that note and save it before planning its task");
          }
          const key = noteReferenceKey(reference);
          if (present.includes(key)) { already.push(key); continue; }
          present.push(key);
          changed.push(key);
        }
        lines.push(planLine(addition));
      }
      if (!lines.length) return { note: note!, changed, already, previousBody };
      let body: string;
      try { body = insertPlanLines(noteId, previousBody, lines); } catch { throw new Error("That day's note has a code block that never closes. Open it before changing the plan."); }
      const saved = await save(date, expected, body);
      for (const line of parsePlan(noteId, saved.body)?.items ?? []) if (line.kind === "newTask" && !present.includes(keyOf(line))) { present.push(keyOf(line)); changed.push(keyOf(line)); }
      return { note: saved, changed, already, previousBody };
    },
    async removeFromDailyPlan(date, expected, key) {
      const noteId = `daily:${date}`;
      const note = await current(date, expected);
      const section = note ? parsePlan(noteId, note.body) : null;
      const line = section?.items.find((candidate) => keyOf(candidate) === key);
      if (!note || !line) throw new Error("That item is no longer in the plan");
      const saved = await save(date, expected, removePlanLine(note.body, line.start, line.end));
      return { note: saved, changed: [key], already: [], previousBody: note.body };
    },
  };
}
