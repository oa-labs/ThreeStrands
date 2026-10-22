import type { AttendeeResponseStatus, ScheduleEvent } from "./domain";

export function responseLabel(event: ScheduleEvent): string | null {
  switch (event.responseStatus) {
    case "accepted": return "Going";
    case "declined": return "Not going";
    case "tentative": return "Maybe";
    case "needsAction": return "Awaiting response";
    default: return event.canRespond ? "Awaiting response" : null;
  }
}

const ATTENDEE_RESPONSES: [AttendeeResponseStatus, string][] = [
  ["accepted", "Going"], ["declined", "Not going"], ["tentative", "Maybe"], ["needsAction", "Awaiting"],
];

export function attendeeResponseLabel(status: AttendeeResponseStatus | undefined): string | null {
  return ATTENDEE_RESPONSES.find(([candidate]) => candidate === status)?.[1] ?? null;
}

/** Counts of the other attendees' RSVPs, for example "2 going, 1 not going". */
export function attendeeResponseSummary(event: ScheduleEvent): string | null {
  const statuses = (event.attendees ?? []).map((email) => event.attendeeResponses?.[email]);
  const counts = ATTENDEE_RESPONSES.flatMap(([status, label]) => {
    const count = statuses.filter((candidate) => candidate === status).length;
    return count ? [`${count} ${label.toLocaleLowerCase()}`] : [];
  });
  return counts.length ? counts.join(", ") : null;
}
