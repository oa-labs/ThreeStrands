import type { ContactProfile, DailyPlan, DailyPlanItem, Goal, ThreadTask } from "../domain";
import { insertPlanLines, parsePlan, planLine, removePlanLine, type PlanLine } from "../dailyPlan";
import { noteReferenceKey } from "../noteEntities";
import type { MailClient } from "./client";

/** As natively (`MAX_PLAN_ADDITIONS`). */
export const MAX_PLAN_ADDITIONS = 50;

type Sources = {
  tasks(): ThreadTask[];
  goals(): Goal[];
  contacts(): ContactProfile[] | Promise<ContactProfile[]>;
  notes: Pick<MailClient, "getDailyNote" | "saveDailyNote" | "getNote" | "listNoteTasks">;
};

const keyOf = (line: PlanLine) => (line.reference ? noteReferenceKey(line.reference) : `line:${line.start}`);
const taskState = (status: ThreadTask["status"]): DailyPlanItem["state"] => status === "in_progress" ? "inProgress" : status === "completed" ? "done" : status === "cancelled" ? "cancelled" : "open";

/** The day's plan for the browser demo, following the native store's rules. */
export function createDemoPlans(sources: Sources): Pick<MailClient, "dailyPlan" | "addToDailyPlan" | "removeFromDailyPlan"> {
  const resolve = async (date: string, noteId: string, line: PlanLine): Promise<DailyPlanItem> => {
    const item: DailyPlanItem = { key: keyOf(line), kind: line.kind, reference: line.reference, title: line.label, state: "text", marker: line.marker, taskId: null, accountId: null, goal: null, contactId: null, start: line.start, end: line.end };
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
      return {
        ...plan,
        revision: note.updatedAt,
        items: section ? await Promise.all(section.items.map((line) => resolve(date, noteId, line))) : [],
        notToday: section ? await Promise.all(section.notToday.map((line) => resolve(date, noteId, line))) : [],
      };
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
