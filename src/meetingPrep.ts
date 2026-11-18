import type { ReactNode } from "react";
import { mailClient } from "./data/client";
import type { MeetingReadiness, ScheduleEvent } from "./domain";
import { isDeclined } from "./agendaEvents";
import { isWorkingLocation } from "./WorkingLocation";
import { meetingAttendees, meetingEntry, meetingEventReference } from "./meetingNoteEntry";
import { MAX_MEETING_ATTENDEES, type MeetingEntry } from "./noteMeetings";
import { normalizeContactAddress, noteReferenceKey } from "./noteEntities";

/** Most meetings one readiness request may name; matches the native limit. */
export const MAX_READINESS_EVENTS = 50;

/** The meeting's line as Add Note would write it without changes: the
 * event's title and its other attendees, named from contacts. */
export async function defaultMeetingEntry(event: ScheduleEvent, ownAddresses: readonly string[]): Promise<MeetingEntry> {
  const attendees = meetingAttendees(event, ownAddresses).slice(0, MAX_MEETING_ATTENDEES);
  const found = await Promise.all(attendees.map(async (address) => {
    const profiles = await mailClient.listContactProfiles(address, 5);
    const person = profiles.find((profile) => profile.addresses.some((other) => normalizeContactAddress(other) === address));
    return [address, person?.displayName?.trim() ?? ""] as const;
  }));
  const names = new Map(found.filter(([, label]) => label));
  const contactIds = attendees.length ? await mailClient.resolveContactIds(attendees) : {};
  return meetingEntry(event, { name: event.title, attendees, names, contactIds, linkNote: false });
}

/** An event that can be prepared and noted: one a note can link, that isn't
 * a working location such as "Home" or "Office". */
export function isPreparable(event: ScheduleEvent): boolean {
  return Boolean(meetingEventReference(event)) && !isWorkingLocation(event);
}

/** The day's meetings a plan or Today lists for preparation: preparable ones
 * that aren't declined and haven't ended. */
export function meetingsToPrepare(events: readonly ScheduleEvent[], now: Date): ScheduleEvent[] {
  return events.filter((event) => isPreparable(event) && !isDeclined(event)
    && (event.allDay || new Date(event.end).getTime() > now.getTime()));
}


const count = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

/** One line describing what is ready, as facts rather than a score. */
export function readinessSummary(readiness: MeetingReadiness | undefined): string {
  if (!readiness) return "Checking…";
  if (readiness.error) return "Couldn't check this meeting";
  const parts = [readiness.hasOccurrenceNote ? "Has notes" : "No notes yet"];
  if (readiness.priorOccurrencesWithNotes) parts.push(`${count(readiness.priorOccurrencesWithNotes, "earlier meeting")} with notes`);
  if (readiness.openCommitments) parts.push(count(readiness.openCommitments, "open commitment"));
  if (readiness.candidateCommitments) parts.push(`${count(readiness.candidateCommitments, "possibly related task")}`);
  if (readiness.scopeRequired) parts.push("Choose mail accounts");
  else if (readiness.latestCorrespondenceAt) parts.push(`Last mail ${new Date(readiness.latestCorrespondenceAt).toLocaleDateString(undefined, { month: "short", day: "numeric" })}`);
  return parts.join(" · ");
}

/** Readiness rows by occurrence, for looking up an event's own. */
export function readinessByKey(rows: readonly MeetingReadiness[]): Map<string, MeetingReadiness> {
  return new Map(rows.map((row) => [noteReferenceKey(row.reference), row]));
}


/** The occurrence a preparation view is open for, keyed as its note links are. */
export function preparationKey(event: ScheduleEvent): string | null {
  const reference = meetingEventReference(event);
  return reference ? noteReferenceKey(reference) : null;
}

/** What a workspace needs to host meeting preparation: which meeting is
 * open in it (by `preparationKey`), how to change that, and the view. */
export type MeetingPreparationHost = {
  key: string | null;
  onChange(key: string | null): void;
  render(event: ScheduleEvent, close: () => void): ReactNode;
};
