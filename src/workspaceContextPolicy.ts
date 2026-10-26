import { parseAddress } from "./emailAddress";
import { isCalendarAttachment } from "./calendarInvitations";
import { attachmentMeetingTitle } from "./invitationTitle";
import type { ContextQuery, WorkspaceContextTarget, WorkspaceConversation, WorkspaceFile } from "./domain";

export const CONTEXT_POLICY = { rows: 6, files: 30, daysBefore: 90, daysAfter: 14, debounceMs: 150, sourceScore: 100, topicScore: 10, attendeeScore: 5, maxTerms: 12 } as const;
const GENERIC = new Set("the and for with from this that have your about review meeting weekly daily monthly call sync update task prepare discuss discussion notes follow up next plan planning read file files document documents attachment attachments email message reply send finish complete draft new important today tomorrow".split(" "));
export function contextTerms(title: string): string[] {
  return [...new Set(title.toLocaleLowerCase().match(/[\p{L}\p{N}]{3,}/gu) ?? [])].filter((word) => !GENERIC.has(word)).slice(0, CONTEXT_POLICY.maxTerms);
}
export function contextQuery(target: WorkspaceContextTarget, accountIds: string[]): ContextQuery {
  if (target.kind === "task") return { accountIds, threadId: target.task.threadId ?? undefined, terms: contextTerms(`${target.task.title} ${target.task.notes ?? ""}`), attendees: [] };
  const date = new Date(target.event.start);
  const at = (days: number) => new Date(date.getTime() + days * 86_400_000).toISOString();
  return { accountIds, terms: contextTerms(target.event.title), attendees: (target.event.attendees ?? []).map((email) => email.toLocaleLowerCase()), after: at(-CONTEXT_POLICY.daysBefore), before: at(CONTEXT_POLICY.daysAfter) };
}
export function rankContext(candidates: WorkspaceConversation[], query: ContextQuery): WorkspaceConversation[] {
  const ranked = candidates.filter((item) => query.accountIds.includes(item.accountId)).flatMap((item) => {
    const direct = item.threadId === query.threadId;
    const words = new Set(item.subject.toLocaleLowerCase().match(/[\p{L}\p{N}]+/gu) ?? []);
    const topics = query.terms.filter((term) => words.has(term)).length;
    const attendee = item.participants.some((address) => query.attendees.includes(parseAddress(address).email.toLocaleLowerCase()));
    if (!direct && !topics) return [];
    // Meetings require topic + participant evidence; a person's unrelated files
    // never become meeting materials merely because they are attending.
    if (!direct && query.attendees.length && !attendee) return [];
    const score = (direct ? CONTEXT_POLICY.sourceScore : 0) + topics * CONTEXT_POLICY.topicScore + (attendee ? CONTEXT_POLICY.attendeeScore : 0);
    return [{ ...item, score, reason: direct ? "From source conversation" : attendee ? "Matching topic and attendees" : "Matching topic" }];
  });
  ranked.sort((a,b) => b.score-a.score || b.sentAt.localeCompare(a.sentAt) || a.threadId.localeCompare(b.threadId));
  return ranked.slice(0, CONTEXT_POLICY.rows);
}
/**
 * A meeting's files keep one invitation: the one titled like the meeting, or
 * else the newest. A series sends a new invitation for every occurrence, and
 * the earlier ones stay reachable from their conversations. Starred files stay.
 */
export function meetingFiles(files: WorkspaceFile[], title: string): WorkspaceFile[] {
  const invitations = files.filter((file) => !file.starred && isCalendarAttachment(file.attachment));
  if (invitations.length < 2) return files;
  const wanted = title.trim().toLocaleLowerCase();
  const kept = invitations.find((file) => attachmentMeetingTitle(file)?.toLocaleLowerCase() === wanted)
    ?? invitations.reduce((newest, file) => file.sentAt > newest.sentAt ? file : newest);
  return files.filter((file) => file === kept || !invitations.includes(file));
}
/** The reason every row shares, said once for the section rather than on each row. */
export function sharedReason(items: { reason?: string }[]): string | null {
  const reason = items[0]?.reason;
  return reason && items.every((item) => item.reason === reason) ? reason : null;
}
