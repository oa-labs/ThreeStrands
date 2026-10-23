import type { ActionProposal, Message, MessageAttachment, ThreadDetail } from "./domain";

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
