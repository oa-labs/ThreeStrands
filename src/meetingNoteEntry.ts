import type { ScheduleEvent } from "./domain";
import { formatClockTime } from "./agendaEvents";
import { eventDate } from "./calendarTime";
import { isEmailAddress } from "./emailAddress";
import { normalizeContactAddress, type NoteBacklink, type NoteReference } from "./noteEntities";
import type { MeetingEntry } from "./noteMeetings";

/** The occurrence a meeting line links, or null for an event without a calendar. */
export function meetingEventReference(event: ScheduleEvent): NoteReference | null {
  if (!event.calendarId) return null;
  const series = event.seriesId && event.seriesId !== event.id ? { series: event.seriesId } : {};
  return { version: 1, kind: "event", id: event.id, account: event.accountId, calendar: event.calendarId, ...series };
}

/** The event's start day in this device's time zone, `YYYY-MM-DD`: the daily note it goes in. */
export function meetingDate(event: ScheduleEvent): string {
  const start = eventDate(event);
  return `${start.getFullYear()}-${String(start.getMonth() + 1).padStart(2, "0")}-${String(start.getDate()).padStart(2, "0")}`;
}

/** The time label the line opens with. */
export function meetingTime(event: ScheduleEvent): string {
  return event.allDay ? "All day" : formatClockTime(eventDate(event));
}

/** The event's other people, without the user's own addresses, in order and once each. */
export function meetingAttendees(event: ScheduleEvent, ownAddresses: readonly string[]): string[] {
  const own = new Set(ownAddresses.map(normalizeContactAddress));
  return [...new Set((event.attendees ?? []).map(normalizeContactAddress))].filter((address) => isEmailAddress(address) && !own.has(address));
}

/** The entry the dialog's choices describe. `contactIds` holds saved contacts' ids by address. */
export function meetingEntry(event: ScheduleEvent, choice: { name: string; attendees: readonly string[]; names: ReadonlyMap<string, string>; contactIds: Readonly<Record<string, string>>; linkNote: boolean }): MeetingEntry {
  const reference = meetingEventReference(event);
  if (!reference) throw new Error("This event can't be added to a note");
  return {
    event: reference,
    time: meetingTime(event),
    name: choice.name,
    linkNote: choice.linkNote,
    attendees: choice.attendees.map((raw) => {
      const address = normalizeContactAddress(raw);
      const contact = choice.contactIds[address];
      return { label: choice.names.get(address) ?? address, reference: { version: 1, kind: "contact", id: address, ...(contact && !contact.startsWith("derived:") ? { contact } : {}) } };
    }),
  };
}

/** Prefer a dedicated meeting note over its daily-note entry. Existing links
 * remain available in the Notes section when more than one note links here. */
export function primaryMeetingNote(rows: readonly NoteBacklink[]): NoteBacklink | undefined {
  return [...rows].sort((a, b) => Number(Boolean(a.dailyDate)) - Number(Boolean(b.dailyDate))
    || (b.dailyDate ?? "").localeCompare(a.dailyDate ?? "") || a.noteId.localeCompare(b.noteId))[0];
}
