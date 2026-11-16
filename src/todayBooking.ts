import { formatClockTime } from "./agendaEvents";
import { revalidateScheduleCache } from "./calendarScheduleCache";
import { mailClient } from "./data/client";
import type { CalendarAccount, CalendarOption, CreateCalendarEventRequest, ScheduleEvent } from "./domain";
import { SLOT_MINUTES } from "./DayTimeGrid";
import { errorMessage } from "./errors";
import { bookedSource, defaultEventCalendar, showCalendar, sourceTaskId, TASK_BLOCK_MINUTES, writableCalendars, type BookedSource } from "./taskBooking";
import { planCapacity, type WorkingHours } from "./todayInsights";
import { schedulingDay } from "./meetingAvailability";
import type { Notice } from "./useNotice";

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
  const source = bookedSource(target.sourceTaskId);
  return {
    accountId: calendar.accountId,
    calendarId: calendar.id,
    title: target.title.trim(),
    start: start.toISOString(),
    end: new Date(start.getTime() + TASK_BLOCK_MINUTES * 60_000).toISOString(),
    description: "",
    attendees: [],
    // Stored as Calendar stores it, so both name the same booked task.
    ...(source ? { sourceTaskId: sourceTaskId(source) } : {}),
  };
}

/**
 * Where **Book** puts a block today: the first `SLOT_MINUTES` step, at or
 * after now, inside a free stretch of today's working hours (`planCapacity`)
 * that still holds `blockMinutes`. `null` when no such stretch is left, and
 * "unknown" when the schedule or the working hours are, since unknown time
 * is never offered as free.
 */
export function nextFreeSlot(events: readonly ScheduleEvent[] | null, workingHours: WorkingHours | null, now: Date, blockMinutes = TASK_BLOCK_MINUTES): Date | null | "unknown" {
  const capacity = planCapacity(events, workingHours, now);
  if (!capacity) return "unknown";
  const step = SLOT_MINUTES * 60_000;
  for (const stretch of capacity.stretches) {
    const start = Math.ceil(stretch.start.getTime() / step) * step;
    if (start + blockMinutes * 60_000 <= stretch.end.getTime()) return new Date(start);
  }
  return null;
}

/** Whether `start`..`start + blockMinutes` lies wholly in today's working hours; null when they're unknown. */
export function withinWorkingHours(start: Date, workingHours: WorkingHours | null, blockMinutes = TASK_BLOCK_MINUTES): boolean | null {
  // With no events and a clock before the day, each working window of the day is one free stretch.
  if (!workingHours) return null;
  const capacity = planCapacity([], workingHours, new Date(0), schedulingDay(start.getTime(), workingHours.timeZone));
  if (!capacity) return null;
  const end = start.getTime() + blockMinutes * 60_000;
  return capacity.stretches.some((stretch) => stretch.start.getTime() <= start.getTime() && end <= stretch.end.getTime());
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

/** The block booked for a task among today's events, resolving replicated and note-task sources as Calendar does: the next one still to end, else the latest. */
export function bookedFor(taskId: string | null, events: readonly ScheduleEvent[], now: Date): ScheduleEvent | null {
  const source = bookedSource(taskId);
  if (!source) return null;
  const blocks = events.filter((event) => sameSource(bookedSource(event.sourceTaskId), source)).sort((a, b) => Date.parse(a.start) - Date.parse(b.start));
  return blocks.find((event) => Date.parse(event.end) > now.getTime()) ?? blocks.at(-1) ?? null;
}

function sameSource(left: BookedSource | null, right: BookedSource): boolean {
  return left !== null && sourceTaskId(left) === sourceTaskId(right);
}

/**
 * `day`'s working windows in local minutes after midnight, merged, from the
 * default availability profile; null when the working hours are unknown,
 * and empty on a day without any.
 */
export function workingSpans(workingHours: WorkingHours | null, day: Date): { startMinutes: number; endMinutes: number }[] | null {
  if (!workingHours) return null;
  const midnight = new Date(day.getFullYear(), day.getMonth(), day.getDate());
  const capacity = planCapacity([], workingHours, new Date(0), schedulingDay(midnight.getTime() + 12 * 3_600_000, workingHours.timeZone));
  if (!capacity) return null;
  const minutes = (instant: Date) => Math.round((instant.getTime() - midnight.getTime()) / 60_000);
  return capacity.stretches
    .map((stretch) => ({ startMinutes: Math.max(0, minutes(stretch.start)), endMinutes: Math.min(24 * 60, minutes(stretch.end)) }))
    .filter((span) => span.endMinutes > span.startMinutes);
}
