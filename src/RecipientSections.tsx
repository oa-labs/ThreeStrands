// Sections about the people a draft is going to, shared by a new message's panel and a reply's.

import { useRef, useState } from "react";
import { CalendarDays, UserRound } from "lucide-react";
import type { Draft } from "./correspondence";
import type { AvailabilityCandidate, AvailabilityPreferences, ContactProfile } from "./domain";
import { ContextSectionHeader } from "./ContextSections";
import { errorMessage } from "./errors";
import { askFindTimes, FindTimesCard, type FindTimesContext } from "./FindTimesCard";
import { lookaheadRange, planTimeSearch, type SchedulePlan, type SuggestedSearch } from "./scheduling";
import { ICON_SIZE } from "./iconSizes";

/** With several recipients, chooses whose history the panel shows. Left out for one or none. */
export function RecipientChips({ recipients, selectedEmail, onSelect }: {
  recipients: { email: string; name: string | null }[];
  selectedEmail: string;
  onSelect(email: string): void;
}) {
  if (recipients.length < 2) return null;
  return (
    <div className="compose-recipient-chips" role="group" aria-label="Show history with">
      {recipients.map((recipient) => (
        <button
          type="button"
          className="recipient-history-chip"
          key={recipient.email}
          aria-pressed={recipient.email === selectedEmail}
          title={recipient.email}
          onClick={() => onSelect(recipient.email)}
        >{recipient.name || recipient.email}</button>
      ))}
    </div>
  );
}

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
export function AvailabilitySection({ preferences, ai, draft, suggest, onInsertTimes, onOpenCalendarSettings }: {
  preferences: AvailabilityPreferences;
  /** Draft Assist is on and an AI provider is ready. */
  ai: boolean;
  /** The draft the times are for, read when Find Times is pressed. */
  draft: Pick<Draft, "sourceId" | "subject" | "body">;
  /** The search a meeting suggestion already found for this conversation, if any. */
  suggest?: () => SuggestedSearch | null;
  onInsertTimes(candidates: AvailabilityCandidate[]): void;
  onOpenCalendarSettings(): void;
}) {
  const [search, setSearch] = useState<FindTimesState | null>(null);
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

  return (
    <section className="context-section compose-availability" aria-label="Availability">
      <ContextSectionHeader
        title="Availability"
        icon={<CalendarDays size={ICON_SIZE.xs} />}
        actions={<button type="button" className="btn btn-sm" disabled={reading} onClick={() => void findTimes()}>
          {search ? "Search Again" : "Find Times"}
        </button>}
      />
      {reading ? <p className="context-status" role="status">Reading the conversation…</p> : null}
      {search && !reading ? <>
        {search.fallback ? <p className="context-status" role="alert">{search.fallback}</p> : null}
        <FindTimesCard
          key={search.key}
          plan={search.plan}
          reason={search.reason}
          preferences={preferences}
          calendarConnected
          ai={ai ? context : null}
          intoDraft
          // A range search never yields a single proposed time to add or confirm.
          onAddToCalendar={() => undefined}
          onReplyWithTimes={onInsertTimes}
          onConfirmTime={() => undefined}
          onOpenCalendarSettings={onOpenCalendarSettings}
        />
      </> : reading ? null : ai ? <>
        <p className="context-status">Suggest open times from your calendar, based on what this conversation asks for.</p>
        <p className="context-section-note">Find Times shares this conversation and your draft text with your AI provider. Your calendar is never sent.</p>
      </> : <p className="context-status">Suggest open times from your calendar for this message.</p>}
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
