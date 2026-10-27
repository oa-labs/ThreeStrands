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

/** Selects a hidden calendar first, so the event created on it shows in the schedule. */
export async function showCalendar(calendar: CalendarOption, calendars: CalendarOption[]): Promise<void> {
  if (calendar.selected) return;
  await mailClient.setCalendarSelection(calendar.accountId, [
    ...calendars.filter((option) => option.accountId === calendar.accountId && option.selected).map((option) => option.id),
    calendar.id,
  ]);
}

/** The To Do and In Progress tasks of these accounts, in board order. */
export function openTasksFor(tasks: ThreadTask[], accountIds: string[]): ThreadTask[] {
  return tasks
    .filter((task) => isActiveTaskStatus(task.status) && accountIds.includes(task.accountId))
    .sort(compareTasksForDisplay);
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
    sourceTaskId: task.id,
  };
}

/** The block booked for the task among `events`: the next one still to end, else the latest. */
export function bookedBlock(task: ThreadTask, events: ScheduleEvent[], now: Date): ScheduleEvent | null {
  const blocks = events.filter((event) => event.sourceTaskId === task.id)
    .sort((a, b) => Date.parse(a.start) - Date.parse(b.start));
  return blocks.find((event) => Date.parse(event.end) > now.getTime()) ?? blocks.at(-1) ?? null;
}
