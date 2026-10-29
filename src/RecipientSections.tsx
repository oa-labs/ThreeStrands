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
import { lookaheadRange, planTimeSearch, type SchedulePlan, type SuggestedSearch } from "./scheduling";
import { ICON_SIZE } from "./iconSizes";

type FindTimesState = {
  /** Each search remounts the card. */
  key: number;
  plan: SchedulePlan;
  /** Why these days; null for the plain coming-week search. */
  reason: string | null;
  /** Shown when AI could not read the conversation and the coming week is used instead. */
  fallback: string | null;
};

/**
 * Open times from the calendar, inserted into the draft at the caret. The
 * first Find Times reuses a meeting suggestion's date range when the
 * conversation already has one. Otherwise, with AI, it reads the
 * conversation and the draft for which days and hours to search; without
 * it, the coming week is searched. Search Again always reads afresh. The
 * provider never sees the calendar.
 */
export function AvailabilitySection({ preferences, ai, draft, ownEmails = [], suggest, intoDraft = true, compact = false, offered = true, offeredTimes = null, onInsertTimes, onOpenCalendarSettings }: {
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
  onInsertTimes(candidates: AvailabilityCandidate[]): void;
  onOpenCalendarSettings(): void;
}) {
  const [search, setSearch] = useState<FindTimesState | null>(null);
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

  const show = (plan: SchedulePlan, reason: string | null, fallback: string | null) =>
    setSearch((previous) => ({ key: (previous?.key ?? 0) + 1, plan, reason, fallback }));
  const [seenOffer, setSeenOffer] = useState<OfferedTimes | null>(null);
  useEffect(() => {
    if (!offeredTimes || !offeredTimes.times.length || offeredTimes === seenOffer) return;
    const duration = offeredTimes.times[0].durationMinutes;
    const durationMinutes = duration && offeredTimes.times.every((time) => time.durationMinutes === duration) ? duration : preferences.defaultDurationMinutes;
    setSeenOffer(offeredTimes);
    ++latest.current;
    setReading(false);
    setSearch((previous) => ({ key: (previous?.key ?? 0) + 1,
      plan: { query: null, proposedTimes: offeredTimes.times, durationMinutes, timeZoneAssumed: false },
      reason: "Times offered in this conversation.", fallback: null }));
  }, [offeredTimes, seenOffer, preferences.defaultDurationMinutes]);
  const comingWeek = (): SchedulePlan => ({ query: lookaheadRange(new Date()), durationMinutes: preferences.defaultDurationMinutes, timeZoneAssumed: false });

  const findTimes = async () => {
    const request = ++latest.current;
    const suggested = search ? null : suggest?.() ?? null;
    if (suggested) {
      show(suggested.plan, suggested.reason, null);
      return;
    }
    if (!ai) {
      show(comingWeek(), null, null);
      return;
    }
    setReading(true);
    try {
      const result = await askFindTimes(context(), preferences.timeZone, null);
      if (request === latest.current) show(planTimeSearch(result, new Date(), preferences.defaultDurationMinutes), result.reason, null);
    } catch (reason) {
      if (request === latest.current) show(comingWeek(), null, `${errorMessage(reason)} Showing the coming week instead.`);
    } finally {
      if (request === latest.current) setReading(false);
    }
  };

  const explanation = ai
    ? "Suggest open times from your calendar, based on what this conversation asks for."
    : "Suggest open times from your calendar for this message.";
  const button = (
    <button type="button" className="btn btn-sm" disabled={reading} title={compact && !search ? explanation : undefined} onClick={() => void findTimes()}>
      {/* Marked only when the press reads the conversation with AI, not when it reuses a suggestion. */}
      {ai && (search !== null || !suggest?.()) ? <Sparkles size={ICON_SIZE.sm} /> : null}{search ? "Search Again" : "Find Times"}
    </button>
  );
  if (compact && !search && !reading) {
    if (!offered) return null;
    return <section className="context-ai-launcher compose-availability" aria-label="Availability">{button}</section>;
  }

  return (
    <section className="context-section compose-availability" aria-label="Availability">
      <ContextSectionHeader
        title="Availability"
        icon={<CalendarDays size={ICON_SIZE.xs} />}
        actions={button}
      />
      {reading ? <p className="context-status" role="status">Reading the conversation…</p> : null}
      {search && !reading ? <>
        {search.fallback ? <p className="context-status" role="alert">{search.fallback}</p> : null}
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
          preferences={preferences}
          calendarConnected
          coworkers={emails.length && draft.account ? { accountId: draft.account, emails } : undefined}
          ai={ai ? context : null}
          intoDraft={intoDraft}
          // A range search never yields a single proposed time to add or confirm.
          onAddToCalendar={() => undefined}
          onReplyWithTimes={onInsertTimes}
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
