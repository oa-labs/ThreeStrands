import type { ReactNode } from "react";
import { mailClient } from "./data/client";
import type { MeetingReadiness, ScheduleEvent } from "./domain";
import { isDeclined } from "./agendaEvents";
import { meetingAttendees, meetingDate, meetingEntry, meetingEventReference } from "./meetingNoteEntry";
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

/** The day's meetings a plan or Today lists for preparation: those that link
 * a note, aren't declined, aren't working locations, and haven't ended. */
export function meetingsToPrepare(events: readonly ScheduleEvent[], now: Date): ScheduleEvent[] {
  return events.filter((event) => meetingEventReference(event) && !isDeclined(event) && event.eventType !== "workingLocation"
    && (event.allDay || new Date(event.end).getTime() > now.getTime()));
}

const NO_PREP_KEY = "threestrands.meetings.noPrepNeeded";
/** Most "No prep needed" choices kept on this device; the oldest go first. */
export const MAX_NO_PREP_CHOICES = 200;

/** The choice's key: the occurrence and the day it was made for. */
export function noPrepKey(event: ScheduleEvent): string | null {
  const reference = meetingEventReference(event);
  return reference ? `${meetingDate(event)}|${noteReferenceKey(reference)}` : null;
}

export function readNoPrepNeeded(): string[] {
  try {
    const stored: unknown = JSON.parse(localStorage.getItem(NO_PREP_KEY) ?? "[]");
    return Array.isArray(stored) ? stored.filter((value): value is string => typeof value === "string") : [];
  } catch {
    return [];
  }
}

/** Marks or clears "No prep needed" for one occurrence on its day, on this device only. */
export function setNoPrepNeeded(key: string, chosen: boolean): string[] {
  const next = [...readNoPrepNeeded().filter((value) => value !== key), ...(chosen ? [key] : [])].slice(-MAX_NO_PREP_CHOICES);
  try { localStorage.setItem(NO_PREP_KEY, JSON.stringify(next)); } catch { /* Still applies until reload. */ }
  return next;
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
