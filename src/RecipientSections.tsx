// Sections about the people a draft is going to, shared by a new message's panel and a reply's.

import { useState } from "react";
import { CalendarDays } from "lucide-react";
import type { AvailabilityCandidate, AvailabilityPreferences, ContactActivity, ContactProfile } from "./domain";
import { ContextSectionHeader } from "./ContextSections";
import { describeActivity } from "./contactContext";
import { MeetingScheduler, type ScheduleSlot } from "./MeetingScheduler";
import { lookaheadRange } from "./scheduling";
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

/** Open times from the calendar over the next week, inserted into the draft at the caret. */
export function AvailabilitySection({ preferences, onInsertTimes, onAddToCalendar, onMoreTimes, onOpenCalendarSettings }: {
  preferences: AvailabilityPreferences;
  onInsertTimes(candidates: AvailabilityCandidate[]): void;
  onAddToCalendar(slot: ScheduleSlot): void;
  onMoreTimes(day: Date, durationMinutes: number): void;
  onOpenCalendarSettings(): void;
}) {
  // A search starts from the moment it was asked for; each Find Times press is a new one.
  const [search, setSearch] = useState<{ key: number; start: Date } | null>(null);
  return (
    <section className="context-section compose-availability" aria-label="Availability">
      <ContextSectionHeader
        title="Availability"
        icon={<CalendarDays size={ICON_SIZE.xs} />}
        actions={<button type="button" className="btn btn-sm" onClick={() => setSearch((current) => ({ key: (current?.key ?? 0) + 1, start: new Date() }))}>
          {search ? "Search Again" : "Find Times"}
        </button>}
      />
      {search ? (
        <MeetingScheduler
          key={search.key}
          plan={{ query: lookaheadRange(search.start), durationMinutes: preferences.defaultDurationMinutes, timeZoneAssumed: false }}
          preferences={preferences}
          calendarConnected
          intoDraft
          onAddToCalendar={onAddToCalendar}
          onReplyWithTimes={onInsertTimes}
          // A lookahead range never yields a single proposed time to confirm.
          onConfirmTime={() => undefined}
          onMoreTimes={onMoreTimes}
          onOpenCalendarSettings={onOpenCalendarSettings}
        />
      ) : <p className="context-status">Suggest open times from your calendar for this message.</p>}
    </section>
  );
}

/** Who the selected recipient is and how much the user has written with them. */
export function RecipientSummary({ email, name, profile, activity }: {
  email: string;
  name: string | null;
  profile: ContactProfile | null;
  activity: ContactActivity | null;
}) {
  const displayName = profile?.displayName || name || email;
  const role = [profile?.role, profile?.company].filter(Boolean).join(" · ");
  const facts = activity ? describeActivity(activity) : [];
  return (
    <section className="context-section compose-recipient" aria-label={`About ${displayName}`}>
      <strong className="compose-recipient-name" title={displayName}>{displayName}</strong>
      {displayName !== email ? <span className="compose-recipient-email" title={email}>{email}</span> : null}
      {role ? <span className="compose-recipient-role">{role}</span> : null}
      {facts.length > 0 ? <ul className="compose-recipient-facts">{facts.map((fact) => <li key={fact}>{fact}</li>)}</ul> : null}
      {profile?.notes?.trim() ? <p className="compose-recipient-notes">{profile.notes.trim()}</p> : null}
    </section>
  );
}
