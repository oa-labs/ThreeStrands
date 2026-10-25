import type { ActionProposal, CalendarEventPreview, Message, MessageAttachment, ThreadDetail } from "./domain";

export function isCalendarAttachment(attachment: MessageAttachment): boolean {
  return attachment.mimeType.split(";", 1)[0]?.trim().toLocaleLowerCase() === "text/calendar"
    || attachment.filename.toLocaleLowerCase().endsWith(".ics");
}

export function latestInvitationMessage(detail: ThreadDetail): Message | null {
  return [...detail.messages].reverse().find((message) => message.attachments.some(isCalendarAttachment)) ?? null;
}

/** Invitations supply their own event; only independent messages may suggest another. */
export function withoutInvitationMeetings(proposals: ActionProposal[], detail: ThreadDetail): ActionProposal[] {
  const invitationIds = new Set(detail.messages.filter((message) => message.attachments.some(isCalendarAttachment)).map((message) => message.id));
  return proposals.filter((proposal) => proposal.type !== "meeting" || !invitationIds.has(proposal.evidence.sourceMessageId));
}

export function parsedInvitationDate(value: string | null): Date | null {
  if (!value) return null;
  const allDay = /^(\d{4})-(\d{2})-(\d{2})$/.exec(value);
  const date = allDay
    ? new Date(Number(allDay[1]), Number(allDay[2]) - 1, Number(allDay[3]))
    : new Date(value);
  return Number.isNaN(date.getTime()) ? null : date;
}

/**
 * The time an invitation event covers, for prefilling a new calendar event.
 * A missing end means one hour (or, all day, one day) after the start.
 * Null when the invitation has no usable start, or its times are wall-clock
 * times in a zone that could not be resolved, which would land at a guess.
 */
export function calendarEventRange(event: CalendarEventPreview): { start: Date; end: Date } | null {
  if (event.timeZone) return null;
  const start = parsedInvitationDate(event.start);
  if (!start) return null;
  const end = parsedInvitationDate(event.end);
  if (end && end > start) return { start, end };
  const fallback = new Date(start);
  if (event.allDay) fallback.setDate(fallback.getDate() + 1);
  else fallback.setHours(fallback.getHours() + 1);
  return { start, end: fallback };
}
