import { isDeclined, formatClockTime } from "./agendaEvents";
import { revalidateScheduleCache } from "./calendarScheduleCache";
import { mailClient } from "./data/client";
import type { CalendarAccount, CalendarOption, CreateCalendarEventRequest, ScheduleEvent } from "./domain";
import { SLOT_MINUTES } from "./DayTimeGrid";
import { errorMessage } from "./errors";
import { defaultEventCalendar, showCalendar, TASK_BLOCK_MINUTES, writableCalendars } from "./taskBooking";
import type { Notice } from "./useNotice";
import { isWorkingLocation } from "./WorkingLocation";

/**
 * What a block of time is booked for on Today: a task (mail, standalone, or a
 * note task as `note:<noteId>:<markerId>`), or nothing, such as a check-in.
 * `accountId` prefers that account's calendars, as Calendar does for a task.
 */
export type BlockTarget = { title: string; sourceTaskId: string | null; accountId: string | null };

/** What booking needs from the app: where events can go and how to say what happened. */
export type BookingContext = {
  accounts: CalendarAccount[];
  calendars: CalendarOption[];
  onNotice(notice: Notice): void;
};

/** A timed block of `TASK_BLOCK_MINUTES`, tagged with its task when it has one. */
export function blockRequest(target: BlockTarget, calendar: CalendarOption, start: Date): CreateCalendarEventRequest {
  return {
    accountId: calendar.accountId,
    calendarId: calendar.id,
    title: target.title.trim(),
    start: start.toISOString(),
    end: new Date(start.getTime() + TASK_BLOCK_MINUTES * 60_000).toISOString(),
    description: "",
    attendees: [],
    ...(target.sourceTaskId ? { sourceTaskId: target.sourceTaskId } : {}),
  };
}

/** Time the user has committed to: timed events not declined, apart from working locations. */
function busy(event: ScheduleEvent): boolean {
  return !event.allDay && !isDeclined(event) && !isWorkingLocation(event);
}

/**
 * Where **Book** puts a block today: the first `SLOT_MINUTES` step at or
 * after now whose `TASK_BLOCK_MINUTES` overlap no busy event and end by
 * midnight, or null when no such stretch is left.
 */
export function nextFreeSlot(events: readonly ScheduleEvent[], now: Date, blockMinutes = TASK_BLOCK_MINUTES): Date | null {
  const day = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  const dayEnd = new Date(day.getFullYear(), day.getMonth(), day.getDate() + 1).getTime();
  const minutes = now.getHours() * 60 + now.getMinutes() + (now.getSeconds() || now.getMilliseconds() ? 1 : 0);
  const taken = events.filter(busy).map((event) => [Date.parse(event.start), Date.parse(event.end)] as const);
  for (let at = Math.ceil(minutes / SLOT_MINUTES) * SLOT_MINUTES; ; at += SLOT_MINUTES) {
    const start = new Date(day.getFullYear(), day.getMonth(), day.getDate(), 0, at).getTime();
    const end = start + blockMinutes * 60_000;
    if (end > dayEnd) return null;
    if (!taken.some(([from, to]) => from < end && to > start)) return new Date(start);
  }
}

/**
 * Books a block for `target` at `start` on the default calendar, showing that
 * calendar first if it is hidden. Returns the event and an undo that deletes
 * it, or null after saying why nothing was booked. The caller says what was
 * booked, since a drop may also have added the item to the plan.
 */
export async function bookBlock(target: BlockTarget, start: Date, context: BookingContext): Promise<{ event: ScheduleEvent; when: string; undo(): Promise<void> } | null> {
  const calendar = defaultEventCalendar(writableCalendars(context.accounts, context.calendars), target.accountId);
  if (!calendar) {
    context.onNotice({ message: "Connect a Google Calendar account with event access to book time" });
    return null;
  }
  try {
    await showCalendar(calendar, context.calendars);
    const event = await mailClient.createCalendarEvent(blockRequest(target, calendar, start));
    revalidateScheduleCache();
    return {
      event,
      when: formatClockTime(new Date(event.start)),
      async undo() {
        await mailClient.deleteCalendarEvent(event);
        revalidateScheduleCache();
      },
    };
  } catch (reason) {
    context.onNotice({ message: errorMessage(reason) });
    return null;
  }
}

/** The block booked for a task among today's events: the next one still to end, else the latest. */
export function bookedFor(taskId: string | null, events: readonly ScheduleEvent[], now: Date): ScheduleEvent | null {
  if (!taskId) return null;
  const blocks = events.filter((event) => event.sourceTaskId === taskId).sort((a, b) => Date.parse(a.start) - Date.parse(b.start));
  return blocks.find((event) => Date.parse(event.end) > now.getTime()) ?? blocks.at(-1) ?? null;
}
