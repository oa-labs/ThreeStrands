import { CheckSquare, ClipboardList, Mail, NotebookPen, NotebookText, X } from "lucide-react";
import { useRef, useState, type FormEvent, type ReactNode } from "react";
import { Modal } from "./AppChrome";
import { ContextRow, ContextSection, ContextSectionHeader } from "./ContextSections";
import { formatEventTime } from "./calendarTime";
import type { MeetingCommitment, MeetingConversation, MeetingNoteSource, MeetingOccurrence, MeetingSection, ScheduleEvent } from "./domain";
import { ICON_SIZE } from "./iconSizes";
import { meetingEventReference } from "./meetingNoteEntry";
import { noteReferenceHref, type NoteReference } from "./noteEntities";
import { notePlainText } from "./notePreview";
import { useMeetingPreparation } from "./useMeetingPreparation";

export type MailAccountChoice = { email: string; label: string };

type Props = {
  event: ScheduleEvent;
  /** Mail accounts on this device, for choosing where to look for related mail. */
  mailAccounts: readonly MailAccountChoice[];
  /** Changes when mail or tasks change, so the view reloads. */
  refreshKey?: unknown;
  /** Opens a note; `href` selects that link in it. */
  onOpenNote(noteId: string, href?: string): void;
  onOpenThread(threadId: string): void;
  onOpenTask(commitment: MeetingCommitment): void;
  onClose(): void;
  /** What to do next for this meeting, such as Add Note and Join, under the title. */
  actions?: ReactNode;
  /** Offered where the meeting has no notes yet. */
  onAddNote?(): void;
};

const CONVERSATION_REASON: Record<MeetingConversation["reason"], string> = {
  origin: "This invitation came from here",
  title: "Names this meeting",
  attendees: "",
};
const COMMITMENT_ORIGIN: Record<MeetingCommitment["origin"], string> = {
  meetingNote: "In this meeting's notes",
  originConversation: "On the invitation's conversation",
  titleConversation: "On a conversation about this meeting",
  attendeeConversation: "On a conversation with an attendee",
};

const shortDate = (iso: string) => new Date(iso).toLocaleDateString(undefined, { month: "short", day: "numeric" });
const dayLabel = (date: string) => new Date(`${date}T00:00:00`).toLocaleDateString(undefined, { weekday: "short", month: "short", day: "numeric" });

/** The section's own explanation: why it is empty, unavailable, or cut. */
function sectionStatus(section: MeetingSection<unknown> | undefined, empty: string): ReactNode {
  if (!section) return null;
  if (section.status === "unavailable") return <p className="context-status">{section.message}</p>;
  return <>
    {section.items.length === 0 && section.status !== "scopeRequired" ? <p className="context-status">{empty}</p> : null}
    {section.omitted > 0 ? <p className="context-status">{section.omitted} more not shown.</p> : null}
  </>;
}

function sourceDetail(source: MeetingNoteSource): string {
  const text = notePlainText(source.text.replace(/<!--[\s\S]*?-->/g, "")).replace(/\s+/g, " ").trim();
  const prefix = source.kind === "reference" ? "Mentioned: " : source.kind === "linkedNote" ? "Meeting note: " : "";
  return `${prefix}${text}${source.truncated ? "…" : ""}`;
}

function noteTitle(source: MeetingNoteSource): string {
  return source.dailyDate ? dayLabel(source.dailyDate) : source.title;
}

/**
 * Read-only preparation for one meeting: its own notes, earlier meetings in
 * its series, related mail, and open commitments, each opening its source.
 * Opening it reads only local data; nothing is written, and nothing takes
 * focus on its own.
 */
export function MeetingPreparation({ event, mailAccounts, refreshKey, onOpenNote, onOpenThread, onOpenTask, onClose, actions, onAddNote }: Props) {
  const reference = meetingEventReference(event);
  const prep = useMeetingPreparation(reference, refreshKey);
  const [choosingScope, setChoosingScope] = useState(false);
  const context = prep.context;
  const occurrenceHref = reference ? noteReferenceHref(reference) : undefined;
  const openSource = (source: MeetingNoteSource, link?: NoteReference) =>
    onOpenNote(source.noteId, source.kind === "linkedNote" ? undefined : link ? noteReferenceHref(link) : occurrenceHref);

  const sourceRows = (sources: readonly MeetingNoteSource[], keyPrefix: string, link?: NoteReference) => sources.map((source, index) => <ContextRow
    key={`${keyPrefix}:${source.noteId}:${source.start}:${index}`}
    icon={source.kind === "linkedNote" ? <NotebookPen size={ICON_SIZE.sm} /> : <NotebookText size={ICON_SIZE.sm} />}
    title={noteTitle(source)}
    detail={sourceDetail(source)}
    onActivate={() => openSource(source, link)}
  />);
  const occurrenceRows = (items: readonly MeetingOccurrence[]) => items.map((item) => {
    const first = item.sources[0];
    const when = item.start ? new Date(item.start).toLocaleDateString(undefined, { weekday: "short", month: "short", day: "numeric" }) : item.date ? dayLabel(item.date) : "Earlier meeting";
    return <ContextRow
      key={JSON.stringify([item.reference.id, item.reference.calendar])}
      icon={<NotebookText size={ICON_SIZE.sm} />}
      title={when}
      detail={first ? sourceDetail(first) : "No notes"}
      onActivate={first ? () => openSource(first, item.reference) : undefined}
    />;
  });
  const conversationRows = (items: readonly MeetingConversation[]) => items.map((item) => <ContextRow
    key={item.threadId}
    icon={<Mail size={ICON_SIZE.sm} />}
    title={item.subject || "(No subject)"}
    date={shortDate(item.lastMessageAt)}
    detail={[item.reason === "attendees" ? `With ${item.attendees.join(", ")}` : CONVERSATION_REASON[item.reason], mailAccounts.length > 1 ? item.accountId : null].filter(Boolean).join(" · ")}
    onActivate={() => onOpenThread(item.threadId)}
  />);
  const commitmentRows = (items: readonly MeetingCommitment[]) => items.map((item) => <ContextRow
    key={item.id}
    icon={<CheckSquare size={ICON_SIZE.sm} />}
    title={item.title}
    wrapTitle
    detail={COMMITMENT_ORIGIN[item.origin]}
    onActivate={() => onOpenTask(item)}
  />);

  const definite = context?.commitments.items.filter((item) => !item.candidate) ?? [];
  const candidates = context?.commitments.items.filter((item) => item.candidate) ?? [];
  const conversations = context?.conversations;
  // Mail naming the meeting, or with several of its attendees, leads; mail
  // with a single attendee is often about something else, so it waits below.
  const several = Math.min(2, context?.event?.attendees.length ?? 0);
  const related = conversations?.items.filter((item) => item.reason !== "attendees" || item.attendees.length >= several) ?? [];
  const otherMail = conversations?.items.filter((item) => !related.includes(item)) ?? [];
  const scopeAction = mailAccounts.length > 1 || conversations?.status === "scopeRequired"
    ? <button type="button" className="btn-link" onClick={() => setChoosingScope(true)}>{conversations?.status === "scopeRequired" ? "Choose Accounts…" : "Mail Accounts…"}</button>
    : null;

  return <section className="meeting-preparation" aria-label={`Preparation for ${event.title}`} aria-busy={prep.loading}>
    <ContextSectionHeader
      title="Preparation"
      icon={<ClipboardList size={ICON_SIZE.xs} />}
      actions={<button type="button" className="btn-icon btn-icon-sm" aria-label="Close Preparation" title="Close Preparation" onClick={onClose}><X size={ICON_SIZE.sm} aria-hidden="true" /></button>}
    />
    <p className="context-section-note">{event.title} · {formatEventTime(event)}{context?.event?.attendees.length ? ` · ${context.event.attendees.join(", ")}` : ""}</p>
    {actions ? <div className="meeting-prep-actions" role="group" aria-label="Meeting actions">{actions}</div> : null}
    {prep.loading && !context ? <p className="context-status" role="status">Loading preparation…</p> : null}
    {prep.error ? <p className="context-status" role="alert">{prep.error} <button type="button" className="btn-link" onClick={prep.reload}>Try Again</button></p> : null}
    {context ? <>
      {!context.event ? <p className="context-status">This meeting isn't in this device's calendar cache, so only its notes are shown.</p> : null}
      <ContextSection id="meeting-prep-this" title="This meeting" count={context.thisMeeting.items.length}
        rows={sourceRows(context.thisMeeting.items, "this")}
        footer={<>
          {sectionStatus(context.thisMeeting, "No notes for this meeting yet.")}
          {onAddNote && context.thisMeeting.items.length === 0 && context.thisMeeting.status !== "unavailable"
            ? <p className="context-status"><button type="button" className="btn-link" onClick={onAddNote}>Add Note</button> to start this meeting's notes in its day's note.</p> : null}
        </>} />
      <ContextSection id="meeting-prep-previous" title="Previous meetings" count={context.previousMeetings.items.length}
        rows={occurrenceRows(context.previousMeetings.items)}
        footer={sectionStatus(context.previousMeetings, context.event?.seriesId ? "No notes from earlier meetings in this series." : "This meeting doesn't repeat.")} />
      {context.seriesNotes.items.length || context.seriesNotes.status === "unavailable" ? <ContextSection id="meeting-prep-series" title="Series notes" count={context.seriesNotes.items.length}
        rows={sourceRows(context.seriesNotes.items, "series")}
        footer={sectionStatus(context.seriesNotes, "")} /> : null}
      <ContextSection id="meeting-prep-mail" title="Conversations" count={related.length}
        actions={scopeAction}
        note={context.mailScope.length ? `Looking in ${context.mailScope.join(", ")}, last ${context.limits.mailLookbackDays} days` : undefined}
        rows={conversationRows(related)}
        footer={<>
          {conversations?.status === "scopeRequired" ? <p className="context-status">{conversations.message}</p> : null}
          {conversations?.status === "unavailable" ? <p className="context-status">{conversations.message}</p> : null}
          {conversations && conversations.status !== "unavailable" && conversations.status !== "scopeRequired" && related.length === 0
            ? <p className="context-status">{otherMail.length ? "No mail about this meeting or with several of its attendees." : "No recent mail about this meeting or with its attendees."}</p> : null}
        </>} />
      {otherMail.length || conversations?.omitted ? <ContextSection id="meeting-prep-other-mail" title="Other mail with attendees" count={otherMail.length} defaultCollapsed
        rows={conversationRows(otherMail)}
        footer={conversations?.omitted ? <p className="context-status">{conversations.omitted} more not shown.</p> : null} /> : null}
      <ContextSection id="meeting-prep-commitments" title="Open commitments" count={definite.length}
        rows={commitmentRows(definite)}
        footer={<>
          {context.commitments.message ? <p className="context-status">{context.commitments.message}</p> : null}
          {sectionStatus({ ...context.commitments, items: definite }, "No open commitments in this meeting's notes or invitation.")}
        </>} />
      {candidates.length ? <ContextSection id="meeting-prep-candidates" title="Possibly related" count={candidates.length}
        note="Open tasks on conversations with attendees. They may not be about this meeting."
        rows={commitmentRows(candidates)} /> : null}
    </> : null}
    {choosingScope ? <MeetingMailScopeDialog
      accounts={mailAccounts}
      chosen={context?.mailScope ?? []}
      onClose={() => setChoosingScope(false)}
      onChoose={(accounts) => { prep.setMailScope(accounts); setChoosingScope(false); }}
    /> : null}
  </section>;
}

/** Choose which mail accounts preparation looks in for related mail. */
export function MeetingMailScopeDialog({ accounts, chosen, onChoose, onClose }: {
  accounts: readonly MailAccountChoice[];
  chosen: readonly string[];
  onChoose(accounts: string[]): void;
  onClose(): void;
}) {
  const [selected, setSelected] = useState(() => new Set(chosen));
  const first = useRef<HTMLInputElement>(null);
  const submit = (event: FormEvent) => {
    event.preventDefault();
    onChoose(accounts.map((account) => account.email).filter((email) => selected.has(email)));
  };
  return <Modal title="Mail Accounts for This Meeting" initialFocusRef={first} onClose={onClose}>
    <form className="modal-form" onSubmit={submit} onKeyDown={(event) => {
      if ((event.metaKey || event.ctrlKey) && event.key === "Enter") { event.preventDefault(); event.currentTarget.requestSubmit(); }
    }}>
      <p className="modal-form-hint">Preparation looks for recent mail with this meeting's attendees in the accounts you choose. Mail stays on this device.</p>
      {accounts.length ? accounts.map((account, index) => <label key={account.email} className="modal-form-check">
        <input ref={index === 0 ? first : undefined} type="checkbox" checked={selected.has(account.email)} onChange={(change) => setSelected((current) => {
          const next = new Set(current);
          if (change.target.checked) next.add(account.email); else next.delete(account.email);
          return next;
        })} />
        {account.label}
      </label>) : <p className="context-status">No mail accounts are connected on this device.</p>}
      <div className="modal-form-actions">
        <button type="button" className="btn" onClick={onClose}>Cancel</button>
        <button type="submit" className="btn btn-primary" disabled={selected.size === 0}>Look in These Accounts</button>
      </div>
    </form>
  </Modal>;
}
