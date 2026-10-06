import { useEffect, useMemo, useState } from "react";
import { mailClient } from "./data/client";
import type { Draft } from "./correspondence";
import type { Account, AvailabilityCandidate, AvailabilityPreferences, ContactActivity, ContactProfile, ContactTimelineItem, ScheduleEvent, ThreadTask } from "./domain";
import { composeChecks, draftRecipients, knownAddressMap, type ComposeCheck, type KnownCorrespondents } from "./composeChecks";
import { ContactFilesSection, ContextSection, ContextSectionHeader, DomainSection, RecentEmailsSection } from "./ContextSections";
import { describeActivity, KNOWN_ADDRESS_LIMIT } from "./contactContext";
import { ThreadTasks } from "./ThreadTasks";
import { ContactMeetings } from "./ContactMeetings";
import { MeetingScheduler, type ScheduleSlot } from "./MeetingScheduler";
import { lookaheadRange } from "./scheduling";
import { errorMessage, logBackgroundFailure } from "./errors";

/**
 * Every address the user has corresponded with, per account, loaded once
 * while a draft is open. Null until loaded or when loading failed, which
 * leaves out the checks that need history.
 */
export function useKnownCorrespondents(accountEmails: string[]): KnownCorrespondents | null {
  const key = accountEmails.map((email) => email.toLocaleLowerCase()).sort().join("\n");
  const [known, setKnown] = useState<KnownCorrespondents | null>(null);
  useEffect(() => {
    const emails = key ? key.split("\n") : [];
    let active = true;
    Promise.all(emails.map((email) => mailClient.listContactSuggestions(email, "", KNOWN_ADDRESS_LIMIT)))
      .then((lists) => { if (active) setKnown(Object.fromEntries(emails.map((email, index) => [email, knownAddressMap(lists[index])]))); })
      .catch((reason: unknown) => {
        logBackgroundFailure("Known correspondent lookup")(reason);
        if (active) setKnown(null);
      });
    return () => { active = false; };
  }, [key]);
  return known;
}

function checkRow(check: ComposeCheck, actions: {
  onAttach(): void;
  onReplaceRecipient(from: string, to: string): void;
  onSwitchAccount(email: string): void;
}) {
  switch (check.kind) {
    case "attachment":
      return <div className="compose-check compose-check-warning" key="attachment">
        <span>Says &ldquo;{check.word}&rdquo;, but nothing is attached</span>
        <button type="button" onClick={actions.onAttach}>Attach Files</button>
      </div>;
    case "subject":
      return <div className="compose-check compose-check-warning" key="subject"><span>No subject</span></div>;
    case "typo": {
      const replacement = check.suggestionName ? `${check.suggestionName} <${check.suggestion}>` : check.suggestion;
      return <div className="compose-check compose-check-warning" key={`typo:${check.email}`}>
        <span>You&rsquo;ve never emailed <strong>{check.email}</strong>. Did you mean <strong>{check.suggestion}</strong>?</span>
        <button type="button" aria-label={`Use ${check.suggestion} instead of ${check.email}`} onClick={() => actions.onReplaceRecipient(check.email, replacement)}>Use {check.suggestion}</button>
      </div>;
    }
    case "firstContact":
      return <div className="compose-check" key={`first:${check.email}`}><span>First email to <strong>{check.email}</strong></span></div>;
    case "outside":
      return <div className="compose-check compose-check-warning" key="outside">
        <span>{check.emails.length === 1 ? "Includes someone" : `Includes ${check.emails.length} people`} outside {check.domain}: {check.emails.join(", ")}</span>
      </div>;
    case "account":
      return <div className="compose-check compose-check-warning" key="account">
        <span>You&rsquo;ve written to {check.emails.join(", ")} from {check.account}</span>
        <button type="button" onClick={() => actions.onSwitchAccount(check.account)}>Send From {check.account}</button>
      </div>;
  }
}

/** Mistakes worth a look before sending, for a new message or a reply. Left out when there are none. */
export function ComposeChecksSection({ draft, accounts, known, onAttach, onReplaceRecipient, onSwitchAccount }: {
  draft: Draft;
  accounts: Account[];
  known: KnownCorrespondents | null;
  onAttach(): void;
  onReplaceRecipient(from: string, to: string): void;
  onSwitchAccount(email: string): void;
}) {
  const ownEmails = useMemo(() => accounts.map((account) => account.email), [accounts]);
  const checks = composeChecks({ draft, ownEmails, known });
  if (checks.length === 0) return null;
  return (
    <ContextSection
      id="compose-checks"
      className="compose-checks"
      title="Before you send"
      count={checks.length}
      rows={checks.map((check) => checkRow(check, { onAttach, onReplaceRecipient, onSwitchAccount }))}
    />
  );
}

/** Open times from the calendar over the next week, inserted into the draft at the caret. */
function AvailabilitySection({ preferences, onInsertTimes, onAddToCalendar, onMoreTimes, onOpenCalendarSettings }: {
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
        actions={<button type="button" className="context-link-button" onClick={() => setSearch((current) => ({ key: (current?.key ?? 0) + 1, start: new Date() }))}>
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
function RecipientSummary({ email, name, profile, activity }: {
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
      <strong className="compose-recipient-name">{displayName}</strong>
      {displayName !== email ? <span className="compose-recipient-email">{email}</span> : null}
      {role ? <span className="compose-recipient-role">{role}</span> : null}
      {facts.length > 0 ? <ul className="compose-recipient-facts">{facts.map((fact) => <li key={fact}>{fact}</li>)}</ul> : null}
      {profile?.notes?.trim() ? <p className="compose-recipient-notes">{profile.notes.trim()}</p> : null}
    </section>
  );
}

/**
 * The context panel while writing a message that is not a reply in the open
 * conversation: checks before sending, the selected recipient and history
 * with them, and open times to offer. It follows the draft's recipients;
 * with several, chips choose whose history to show.
 */
export function ComposeContext({
  draft,
  accounts,
  calendarConnected,
  preferences,
  taskRefreshKey,
  onAttach,
  onReplaceRecipient,
  onSwitchAccount,
  onInsertTimes,
  onAddToCalendar,
  onMoreTimes,
  onOpenCalendarSettings,
  onOpenEvent,
  onOpenThread,
  onShowMessage,
  onEditTask,
  onDraftFollowUp,
  onTasksChanged,
}: {
  draft: Draft;
  accounts: Account[];
  calendarConnected: boolean;
  preferences: AvailabilityPreferences;
  taskRefreshKey: number;
  onAttach(): void;
  onReplaceRecipient(from: string, to: string): void;
  onSwitchAccount(email: string): void;
  onInsertTimes(candidates: AvailabilityCandidate[]): void;
  onAddToCalendar(slot: ScheduleSlot, invitees: string[]): void;
  onMoreTimes(day: Date, durationMinutes: number): void;
  onOpenCalendarSettings(): void;
  /** Shows a meeting without leaving the draft. */
  onOpenEvent(event: ScheduleEvent): void;
  /** Opens a conversation; the caller saves the draft first, since the reader replaces it. */
  onOpenThread(id: string): void;
  onShowMessage(threadId: string, messageId: string): void;
  onEditTask(task: ThreadTask): void;
  onDraftFollowUp(task: ThreadTask): void;
  onTasksChanged(): void;
}) {
  const ownEmails = useMemo(() => accounts.map((account) => account.email), [accounts]);
  const known = useKnownCorrespondents(ownEmails);
  const recipients = draftRecipients(draft, ownEmails);
  const [picked, setPicked] = useState<string | null>(null);
  const selected = recipients.find((recipient) => recipient.email === picked) ?? recipients[0] ?? null;
  const email = selected?.email ?? "";

  const [loaded, setLoaded] = useState<{
    email: string;
    contactId: string;
    profile: ContactProfile | null;
    activity: ContactActivity | null;
    timeline: ContactTimelineItem[];
  } | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    if (!email) return;
    let active = true;
    void (async () => {
      try {
        const owners = await mailClient.resolveContactIds([email]);
        const id = owners[email] ?? `derived:${email}`;
        const profile = await mailClient.getContactProfile(id);
        const contactId = profile?.id ?? id;
        const [activity, timeline] = await Promise.all([
          mailClient.contactActivity(contactId),
          mailClient.contactTimeline(contactId, 0, 5),
        ]);
        if (active) { setLoaded({ email, contactId, profile, activity, timeline }); setError(null); }
      } catch (reason) {
        if (active) setError(errorMessage(reason));
      }
    })();
    return () => { active = false; };
  }, [email]);
  const person = loaded && loaded.email === email ? loaded : null;
  const meetingPeople = recipients.map((recipient) => ({
    email: recipient.email,
    name: (person?.email === recipient.email ? person.profile?.displayName : null) || recipient.name || recipient.email,
  }));
  const invitees = recipients.map((recipient) => recipient.email);

  return (
    <aside className="context-panel compose-context" aria-label="Compose context">
      <ComposeChecksSection
        draft={draft}
        accounts={accounts}
        known={known}
        onAttach={onAttach}
        onReplaceRecipient={onReplaceRecipient}
        onSwitchAccount={onSwitchAccount}
      />
      {recipients.length > 1 ? (
        <div className="compose-recipient-chips" role="group" aria-label="Show history with">
          {recipients.map((recipient) => (
            <button
              type="button"
              key={recipient.email}
              aria-pressed={recipient.email === email}
              title={recipient.email}
              onClick={() => setPicked(recipient.email)}
            >{recipient.name || recipient.email}</button>
          ))}
        </div>
      ) : null}
      {selected ? <RecipientSummary email={selected.email} name={selected.name} profile={person?.profile ?? null} activity={person?.activity ?? null} /> : (
        <p className="context-status compose-context-empty">Add a recipient to see your history with them.</p>
      )}
      {calendarConnected ? (
        <AvailabilitySection
          preferences={preferences}
          onInsertTimes={onInsertTimes}
          onAddToCalendar={(slot) => onAddToCalendar(slot, invitees)}
          onMoreTimes={onMoreTimes}
          onOpenCalendarSettings={onOpenCalendarSettings}
        />
      ) : null}
      {person ? (
        <ThreadTasks
          key={person.contactId}
          thread={null}
          contactId={person.contactId}
          refreshKey={taskRefreshKey}
          onEditTask={onEditTask}
          onDraftFollowUp={onDraftFollowUp}
          onTasksChanged={onTasksChanged}
        />
      ) : null}
      {calendarConnected && meetingPeople.length > 0 ? (
        <ContactMeetings people={meetingPeople} timeZone={preferences.timeZone} onOpenEvent={onOpenEvent} />
      ) : null}
      {person && person.timeline.length > 0 ? <RecentEmailsSection items={person.timeline} onOpenThread={onOpenThread} /> : null}
      {person ? <ContactFilesSection key={person.contactId} contactId={person.contactId} onShowMessage={onShowMessage} /> : null}
      {person ? (
        <DomainSection
          email={person.email}
          addresses={person.profile?.addresses ?? [person.email]}
          accounts={accounts}
          hideThreadIds={person.timeline.map((item) => item.threadId)}
          onOpenThread={onOpenThread}
        />
      ) : null}
      {error ? <p className="contacts-error" role="alert">{error}</p> : null}
    </aside>
  );
}

/** The checks for a reply in the open conversation, above the conversation's own sections. */
export function ReplyChecks(props: Omit<Parameters<typeof ComposeChecksSection>[0], "known">) {
  const ownEmails = useMemo(() => props.accounts.map((account) => account.email), [props.accounts]);
  const known = useKnownCorrespondents(ownEmails);
  return <ComposeChecksSection {...props} known={known} />;
}
