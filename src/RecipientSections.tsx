// Sections about the people a draft is going to, shared by a new message's panel and a reply's.

import { useEffect, useRef, useState } from "react";
import { draftRecipients } from "./composeChecks";
import { isPersonalMailDomain } from "./contactContext";
import { CalendarDays, Sparkles, UserRound } from "lucide-react";
import type { Draft } from "./correspondence";
import type { AvailabilityCandidate, AvailabilityPreferences, ContactProfile, OfferedTimes } from "./domain";
import { ContextSectionHeader } from "./ContextSections";
import { errorMessage } from "./errors";
import { askFindTimes, FindTimesCard, type FindTimesContext } from "./FindTimesCard";
import type { ScheduleSlot } from "./MeetingScheduler";
import { defaultLength, lookaheadRange, planTimeSearch, type SchedulePlan, type SuggestedSearch } from "./scheduling";
import { ICON_SIZE } from "./iconSizes";

type FindTimesState = {
  /** Each search remounts the card. */
  key: number;
  plan: SchedulePlan;
  /** Why these days; null for the plain coming-week search. */
  reason: string | null;
  /** Shown when AI could not read the conversation and the coming week is used instead. */
  fallback: string | null;
  /** The conversation and draft the search was based on, so Re-read is offered only once they change. */
  readFrom: string;
};

/** What Find Times reads, compared to tell whether reading again could find anything new. */
function readKey(context: FindTimesContext): string {
  return JSON.stringify([context.sourceMessageId, context.draftSubject.trim(), context.draftText.trim()]);
}

/** Lets a meeting suggestion point the reader at the times offered here. */
export const AVAILABILITY_SECTION_ID = "availability-section";

/**
 * Open times from the calendar, inserted into the draft at the caret. The
 * first Find Times reuses a meeting suggestion's date range when the
 * conversation already has one. Otherwise, with AI, it reads the
 * conversation and the draft for which days and hours to search; without
 * it, the coming week is searched. Once the conversation or the draft has
 * changed, Re-read reads them afresh; before that it could only repeat the
 * same search, so it is not offered. The provider never sees the calendar.
 */
export function AvailabilitySection({ preferences, ai, draft, ownEmails = [], suggest, intoDraft = true, compact = false, offered = true, offeredTimes = null, offeredTimesError = null, onRetryOfferedTimes, onInsertTimes, onAcceptTime, onOpenCalendarSettings }: {
  preferences: AvailabilityPreferences;
  /** Draft Assist is on and an AI provider is ready. */
  ai: boolean;
  /** The draft the times are for, read when Find Times is pressed. */
  draft: Pick<Draft, "sourceId" | "subject" | "body"> & Partial<Pick<Draft, "id" | "account" | "from" | "to" | "cc">>;
  ownEmails?: string[];
  /** The search a meeting suggestion already found for this conversation, if any. */
  suggest?: () => SuggestedSearch | null;
  /** The times go into an open draft; otherwise choosing them starts a reply. */
  intoDraft?: boolean;
  /**
   * Before a search, only the Find Times button shows, for a row of AI
   * actions; the section opens to full width once there are times.
   */
  compact?: boolean;
  /**
   * Whether the compact button is offered. Once a search has started it
   * stays, so the times never vanish while the user reads them.
   */
  offered?: boolean;
  offeredTimes?: OfferedTimes | null;
  offeredTimesError?: string | null;
  /** Re-read the Brief using its own enabled feature and model. */
  onRetryOfferedTimes?(): Promise<void>;
  onInsertTimes(candidates: AvailabilityCandidate[]): void;
  /** Takes one of the times the conversation offered, when not writing a draft. */
  onAcceptTime?(slot: ScheduleSlot): void;
  onOpenCalendarSettings(): void;
}) {
  const [search, setSearch] = useState<FindTimesState | null>(null);
  const [readError, setReadError] = useState<string | null>(null);
  const sender = (draft.from || draft.account || "").toLowerCase();
  const domain = sender.split("@")[1];
  const coworkers = domain && !isPersonalMailDomain(domain) ? draftRecipients({ to: draft.to ?? "", cc: draft.cc ?? "", bcc: "" }, [...ownEmails, sender, draft.account ?? ""])
    .filter((recipient) => recipient.email.split("@")[1] === domain) : [];
  // Drop removed recipients immediately; adding them again requires opting in again.
  const scope = `${draft.id ?? draft.sourceId ?? ""}|${draft.account ?? ""}|${sender}`;
  const [included, setIncluded] = useState<{ scope: string; emails: string[] }>({ scope, emails: [] });
  const emails = included.scope === scope ? included.emails.filter((email) => coworkers.some((person) => person.email === email)) : [];
  if (included.scope !== scope || included.emails.length !== emails.length) setIncluded({ scope, emails });
  const [reading, setReading] = useState(false);
  // Only the latest request may update the card.
  const latest = useRef(0);
  const draftRef = useRef(draft);
  draftRef.current = draft;
  const context = (): FindTimesContext => ({
    sourceMessageId: draftRef.current.sourceId,
    draftSubject: draftRef.current.subject,
    draftText: draftRef.current.body,
  });

  const show = (plan: SchedulePlan, reason: string | null, fallback: string | null, readFrom: string) =>
    setSearch((previous) => ({ key: (previous?.key ?? 0) + 1, plan, reason, fallback, readFrom }));
  // Times this card inserted are not news to read again; the draft's next change is taken as the insertion.
  const inserting = useRef(false);
  const currentRead = readKey({ sourceMessageId: draft.sourceId, draftSubject: draft.subject, draftText: draft.body });
  if (search && search.readFrom !== currentRead && inserting.current) {
    inserting.current = false;
    setSearch({ ...search, readFrom: currentRead });
  }
  const [seenOffer, setSeenOffer] = useState<OfferedTimes | null>(null);
  useEffect(() => {
    if (!offeredTimes && seenOffer) {
      setSeenOffer(null);
      setSearch((previous) => previous?.plan.proposedTimes === seenOffer.times ? null : previous);
      return;
    }
    if (!offeredTimes || !offeredTimes.times.length || offeredTimes === seenOffer) return;
    const duration = offeredTimes.times[0].durationMinutes;
    const stated = duration && offeredTimes.times.every((time) => time.durationMinutes === duration) ? duration : null;
    const durationMinutes = stated ?? preferences.defaultDurationMinutes;
    setSeenOffer(offeredTimes);
    ++latest.current;
    setReading(false);
    setSearch((previous) => ({ key: (previous?.key ?? 0) + 1,
      plan: { query: null, proposedTimes: offeredTimes.times, durationMinutes, ...defaultLength(stated), timeZoneAssumed: false },
      reason: "Times offered in this conversation.", fallback: null, readFrom: readKey(context()) }));
  }, [offeredTimes, seenOffer, preferences.defaultDurationMinutes]);
  const comingWeek = (): SchedulePlan => ({ query: lookaheadRange(new Date()), durationMinutes: preferences.defaultDurationMinutes, durationIsDefault: true, timeZoneAssumed: false });

  const findTimes = async () => {
    setReadError(null);
    const request = ++latest.current;
    if (offeredTimesError && onRetryOfferedTimes) {
      setReading(true);
      try { await onRetryOfferedTimes(); }
      catch (reason) { if (request === latest.current) setReadError(errorMessage(reason)); }
      finally { if (request === latest.current) setReading(false); }
      return;
    }
    const suggested = search ? null : suggest?.() ?? null;
    if (suggested) {
      show(suggested.plan, suggested.reason, null, readKey(context()));
      return;
    }
    if (!ai) {
      show(comingWeek(), null, null, readKey(context()));
      return;
    }
    setReading(true);
    // What was read, not what the draft holds when the answer arrives: edits made meanwhile are still news.
    const asked = context();
    try {
      const result = await askFindTimes(asked, preferences.timeZone, null);
      if (request === latest.current) show(planTimeSearch(result, new Date(), preferences.defaultDurationMinutes), result.reason, null, readKey(asked));
    } catch (reason) {
      if (request === latest.current) {
        if (offeredTimesError) setReadError(`${errorMessage(reason)} No proposed times were checked.`);
        else show(comingWeek(), null, `${errorMessage(reason)} Showing the coming week instead.`, readKey(asked));
      }
    } finally {
      if (request === latest.current) setReading(false);
    }
  };

  const readsConversation = ai || Boolean(offeredTimesError && onRetryOfferedTimes);
  const explanation = readsConversation
    ? "Suggest open times from your calendar, based on what this conversation asks for."
    : "Suggest open times from your calendar for this message.";
  // Once there are times, the card's own controls change them; the header starts a search only.
  const button = search ? null : (
    <button type="button" className="btn btn-sm" disabled={reading || Boolean(offeredTimesError && !readsConversation)} title={compact ? explanation : undefined} onClick={() => void findTimes()}>
      {/* Marked only when the press reads the conversation with AI, not when it reuses a suggestion. */}
      {readsConversation && !suggest?.() ? <Sparkles size={ICON_SIZE.sm} /> : null}Find Times
    </button>
  );
  // Reading the same conversation and draft again would repeat the same search, so Re-read waits for a change.
  const reread = ai && search && !search.fallback && search.readFrom !== currentRead
    ? <button type="button" className="btn-link find-times-reread" title="Read the conversation and your draft again for when to meet" onClick={() => void findTimes()}>
      <Sparkles size={ICON_SIZE.xs} aria-hidden="true" />Re-read
    </button>
    : null;
  if (compact && !search && !reading && !offeredTimesError && !readError) {
    if (!offered) return null;
    return <section className="context-ai-launcher compose-availability" aria-label="Availability">{button}</section>;
  }

  return (
    <section id={AVAILABILITY_SECTION_ID} className="context-section compose-availability" aria-label="Availability">
      <ContextSectionHeader
        title="Availability"
        icon={<CalendarDays size={ICON_SIZE.xs} />}
        actions={button}
      />
      {!search && !reading && (readError ?? offeredTimesError) ? <p className="context-status" role="alert">{readError ?? offeredTimesError}</p> : null}
      {reading ? <p className="context-status" role="status">Reading the conversation…</p> : null}
      {search && !reading ? <>
        {search.fallback ? <p className="context-status" role="alert">
          {search.fallback} <button type="button" className="btn-link" onClick={() => void findTimes()}>Try Again</button>
        </p> : null}
        {!search.plan.proposedTimes?.length && coworkers.length > 0 ? <fieldset className="find-times-coworkers">
          <legend>Include coworkers</legend>
          {coworkers.map((person) => <label key={person.email}>
            <input type="checkbox" checked={emails.includes(person.email)} onChange={(event) => {
              setIncluded({ scope, emails: event.target.checked ? [...emails, person.email] : emails.filter((email) => email !== person.email) });
            }} />
            <span>{person.name ? `${person.name} (${person.email})` : person.email}</span>
          </label>)}
        </fieldset> : null}
        <FindTimesCard
          key={search.key}
          plan={search.plan}
          reason={search.reason}
          reasonAction={reread}
          preferences={preferences}
          calendarConnected
          coworkers={emails.length && draft.account ? { accountId: draft.account, emails } : undefined}
          ai={ai ? context : null}
          intoDraft={intoDraft}
          // A range search never yields a single proposed time to add or confirm.
          onAddToCalendar={() => undefined}
          onReplyWithTimes={(times) => { inserting.current = true; onInsertTimes(times); }}
          onAcceptTime={onAcceptTime}
          onConfirmTime={() => undefined}
          onOpenCalendarSettings={onOpenCalendarSettings}
        />
      </> : reading ? null : <p className="context-status">{explanation}</p>}
    </section>
  );
}

/**
 * What the user has saved about the selected recipient: role, company, and
 * notes. Left out when nothing is saved; volume and dates are the Recent
 * emails section's job.
 */
export function RecipientSummary({ email, name, profile }: {
  email: string;
  name: string | null;
  profile: ContactProfile | null;
}) {
  const displayName = profile?.displayName || name || email;
  const role = [profile?.role, profile?.company].filter(Boolean).join(" · ");
  const notes = profile?.notes?.trim() ?? "";
  if (!role && !notes) return null;
  return (
    <section className="context-section compose-recipient" aria-label={`About ${displayName}`}>
      <ContextSectionHeader title="About" icon={<UserRound size={ICON_SIZE.xs} />} />
      {role ? <p className="compose-recipient-role">{role}</p> : null}
      {notes ? <p className="compose-recipient-notes">{notes}</p> : null}
    </section>
  );
}
