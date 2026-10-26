import { isCalendarAttachment } from "./calendarInvitations";
import type { MessageAttachment } from "./domain";

/** Status words calendar systems put before a meeting title in invitation and reply subjects. */
const STATUS_PREFIX = /^(?:(?:re|fw|fwd)\s*:\s*)*(?:updated\s+invitation(?:\s+with\s+note)?|invitation(?:\s+with\s+note)?|(?:new|updated|canceled|cancelled)\s+event|event\s+(?:updated|canceled|cancelled)|tentatively\s+accepted|accepted|declined|tentative|canceled|cancelled|updated)\s*:\s*/i;

/** The meeting title in an invitation email's subject, without the status word
 * before it or the " @ <when>" schedule after it. Null when nothing is left. */
export function invitationTitle(subject: string): string | null {
  let title = subject.trim().replace(STATUS_PREFIX, "");
  // An address has no spaces around its @, so " @ " followed by a date starts the schedule.
  const at = title.lastIndexOf(" @ ");
  if (at > 0 && /\d/.test(title.slice(at))) title = title.slice(0, at);
  title = title.trim();
  return title || null;
}

/** Invitations are named by their meeting in file lists, since the filename (often "invite.ics") says nothing. */
export function attachmentMeetingTitle(file: { subject: string; attachment: MessageAttachment }): string | null {
  return isCalendarAttachment(file.attachment) ? invitationTitle(file.subject) : null;
}
