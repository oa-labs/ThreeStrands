import { mailClient } from "./data/client";
import type { CalendarAccount, CalendarOption, CreateCalendarEventRequest, ScheduleEvent, ThreadTask } from "./domain";
import { compareTasksForDisplay, isActiveTaskStatus } from "./taskViews";

/** How long a block of time booked for a task lasts, in minutes. */
export const TASK_BLOCK_MINUTES = 30;

/** Calendars on connected accounts where the user can create events. */
export function writableCalendars(accounts: CalendarAccount[], calendars: CalendarOption[]): CalendarOption[] {
  return calendars.filter((calendar) =>
    calendar.writable && accounts.some((account) => account.email === calendar.accountId && account.status === "connected"));
}

/**
 * Where a new event goes: a visible calendar before a hidden one, the primary
 * before the rest, and the preferred account's calendars before any other.
 */
export function defaultEventCalendar(writable: CalendarOption[], preferredAccountId?: string | null): CalendarOption | undefined {
  const pick = (options: CalendarOption[]) => {
    const visible = options.filter((calendar) => calendar.selected);
    return visible.find((calendar) => calendar.primary) ?? visible[0] ?? options.find((calendar) => calendar.primary) ?? options[0];
  };
  return (preferredAccountId ? pick(writable.filter((calendar) => calendar.accountId === preferredAccountId)) : undefined) ?? pick(writable);
}

/** Each account's primary calendar first, then the rest, otherwise in listed order. */
export function primaryCalendarsFirst(writable: CalendarOption[]): CalendarOption[] {
  return [...writable.filter((calendar) => calendar.primary), ...writable.filter((calendar) => !calendar.primary)];
}

/** Selects a hidden calendar first, so the event created on it shows in the schedule. */
export async function showCalendar(calendar: CalendarOption, calendars: CalendarOption[]): Promise<void> {
  if (calendar.selected) return;
  await mailClient.setCalendarSelection(calendar.accountId, [
    ...calendars.filter((option) => option.accountId === calendar.accountId && option.selected).map((option) => option.id),
    calendar.id,
  ]);
}

/**
 * The To Do and In Progress tasks of these accounts, in board order. With
 * `includeNoteTasks`, open note tasks (which have no account) can be booked too.
 */
export function openTasksFor(tasks: ThreadTask[], accountIds: string[], { includeNoteTasks = false }: { includeNoteTasks?: boolean } = {}): ThreadTask[] {
  return tasks
    .filter((task) => isActiveTaskStatus(task.status) && (task.source === "note" ? includeNoteTasks : accountIds.includes(task.accountId)))
    .sort(compareTasksForDisplay);
}

/**
 * The task a booked event's source task names: a replicated task id, or a
 * note task as Tasks names it, `note:<noteId>:<markerId>`. Marker ids never
 * contain a colon, so the note id is everything between. Events booked before
 * note tasks could be booked hold replicated ids and keep that meaning.
 */
export type BookedSource = { kind: "task"; id: string } | { kind: "noteTask"; noteId: string; markerId: string };

export function bookedSource(sourceTaskId: string | null | undefined): BookedSource | null {
  const id = sourceTaskId?.trim();
  if (!id) return null;
  const note = /^note:(.+):([^:]+)$/.exec(id);
  return note ? { kind: "noteTask", noteId: note[1], markerId: note[2] } : { kind: "task", id };
}

/** The source a block booked for `task` records. */
export function taskSource(task: ThreadTask): BookedSource {
  if (task.noteSource) return { kind: "noteTask", noteId: task.noteSource.noteId, markerId: task.noteSource.markerId };
  return bookedSource(task.id) ?? { kind: "task", id: task.id };
}

/** The event's `sourceTaskId` value for a block booked for `source`. */
export function sourceTaskId(source: BookedSource): string {
  return source.kind === "noteTask" ? `note:${source.noteId}:${source.markerId}` : source.id;
}

function sameSource(left: BookedSource | null, right: BookedSource): boolean {
  if (!left || left.kind !== right.kind) return false;
  return left.kind === "task" ? left.id === (right as typeof left).id
    : left.noteId === (right as typeof left).noteId && left.markerId === (right as typeof left).markerId;
}

/** The task among `tasks` that a booked event was booked for, mail, standalone, or note task. */
export function bookedTaskFor(event: Pick<ScheduleEvent, "sourceTaskId">, tasks: readonly ThreadTask[]): ThreadTask | undefined {
  const source = bookedSource(event.sourceTaskId);
  return source ? tasks.find((task) => sameSource(source, taskSource(task))) : undefined;
}

/** A timed block of `TASK_BLOCK_MINUTES` for the task, tagged so the calendar can name it later. */
export function taskBlockRequest(task: ThreadTask, calendar: CalendarOption, start: Date): CreateCalendarEventRequest {
  return {
    accountId: calendar.accountId,
    calendarId: calendar.id,
    title: task.title.trim(),
    start: start.toISOString(),
    end: new Date(start.getTime() + TASK_BLOCK_MINUTES * 60_000).toISOString(),
    description: "",
    attendees: [],
    sourceTaskId: sourceTaskId(taskSource(task)),
  };
}

/** The block booked for the task among `events`: the next one still to end, else the latest. */
export function bookedBlock(task: ThreadTask, events: ScheduleEvent[], now: Date): ScheduleEvent | null {
  const source = taskSource(task);
  const blocks = events.filter((event) => sameSource(bookedSource(event.sourceTaskId), source))
    .sort((a, b) => Date.parse(a.start) - Date.parse(b.start));
  return blocks.find((event) => Date.parse(event.end) > now.getTime()) ?? blocks.at(-1) ?? null;
}
