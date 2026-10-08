import { useEffect, useMemo, useState, type KeyboardEvent } from "react";
import { mailClient } from "./data/client";
import type { Draft } from "./correspondence";
import type { Account, AvailabilityCandidate, ContactGroupRecipients, AvailabilityPreferences, ContactActivity, ContactProfile, ContactTimelineItem, ScheduleEvent, ThreadTask } from "./domain";
import { composeChecks, draftRecipients, knownAddressMap, type ComposeCheck, type KnownCorrespondents } from "./composeChecks";
import { ContactFilesSection, ContextSection, DomainSection, RecentEmailsSection } from "./ContextSections";
import { AvailabilitySection, RecipientChips, RecipientSummary } from "./RecipientSections";
import { KNOWN_ADDRESS_LIMIT } from "./contactContext";
import { ThreadTasks } from "./ThreadTasks";
import { ContactMeetings } from "./ContactMeetings";
import type { ScheduleSlot } from "./MeetingScheduler";
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

/** Every contact group with its members' addresses, loaded once while a draft is open. Null until loaded. */
export function useContactGroupRecipients(): ContactGroupRecipients[] | null {
  const [groups, setGroups] = useState<ContactGroupRecipients[] | null>(null);
  useEffect(() => {
    let active = true;
    mailClient.listContactGroupRecipients()
      .then((result) => { if (active) setGroups(result); })
      .catch(logBackgroundFailure("Contact group lookup"));
    return () => { active = false; };
  }, []);
  return groups;
}

function checkRow(check: ComposeCheck, actions: {
  onAttach(): void;
  onReplaceRecipient(from: string, to: string): void;
  onSwitchAccount(email: string): void;
  onMoveToBcc(emails: string[]): void;
}) {
  switch (check.kind) {
    case "attachment":
      return <div className="compose-check compose-check-warning" key="attachment">
        <span>Says &ldquo;{check.word}&rdquo;, but nothing is attached</span>
        <button type="button" className="btn btn-sm btn-wrap" onClick={actions.onAttach}>Attach Files</button>
      </div>;
    case "subject":
      return <div className="compose-check compose-check-warning" key="subject"><span>No subject</span></div>;
    case "typo": {
      const replacement = check.suggestionName ? `${check.suggestionName} <${check.suggestion}>` : check.suggestion;
      return <div className="compose-check compose-check-warning" key={`typo:${check.email}`}>
        <span>You&rsquo;ve never emailed <strong>{check.email}</strong>. Did you mean <strong>{check.suggestion}</strong>?</span>
        <button type="button" className="btn btn-sm btn-wrap" aria-label={`Use ${check.suggestion} instead of ${check.email}`} onClick={() => actions.onReplaceRecipient(check.email, replacement)}>Use {check.suggestion}</button>
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
        <button type="button" className="btn btn-sm btn-wrap" onClick={() => actions.onSwitchAccount(check.account)}>Send From {check.account}</button>
      </div>;
    case "groupExposure":
      return <div className="compose-check compose-check-warning" key={`group:${check.group}`}>
        <span>{check.count} people from <strong>{check.group}</strong> are in To or Cc, so each will see everyone else&rsquo;s address</span>
        <button type="button" className="btn btn-sm btn-wrap" aria-label={`Move ${check.group} to Bcc`} onClick={() => actions.onMoveToBcc(check.emails)}>Move to Bcc</button>
      </div>;
  }
}

/** Mistakes worth a look before sending, for a new message or a reply. Left out when there are none. */
export function ComposeChecksSection({ draft, accounts, known, onAttach, onReplaceRecipient, onSwitchAccount, onMoveToBcc }: {
  draft: Draft;
  accounts: Account[];
  known: KnownCorrespondents | null;
  onAttach(): void;
  onReplaceRecipient(from: string, to: string): void;
  onSwitchAccount(email: string): void;
  onMoveToBcc(emails: string[]): void;
}) {
  const ownEmails = useMemo(() => accounts.map((account) => account.email), [accounts]);
  const groups = useContactGroupRecipients();
  const checks = composeChecks({ draft, ownEmails, known, groups });
  if (checks.length === 0) return null;
  return (
    <ContextSection
      id="compose-checks"
      className="compose-checks"
      title="Before you send"
      count={checks.length}
      rows={checks.map((check) => checkRow(check, { onAttach, onReplaceRecipient, onSwitchAccount, onMoveToBcc }))}
    />
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
  onMoveToBcc,
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
  onKeyDown,
}: {
  draft: Draft;
  accounts: Account[];
  calendarConnected: boolean;
  preferences: AvailabilityPreferences;
  taskRefreshKey: number;
  onAttach(): void;
  onReplaceRecipient(from: string, to: string): void;
  onSwitchAccount(email: string): void;
  onMoveToBcc(emails: string[]): void;
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
  /** Keyboard handling for the panel while focus is inside it. */
  onKeyDown?(event: KeyboardEvent<HTMLElement>): void;
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
    <aside className="context-panel compose-context" aria-label="Compose context" tabIndex={-1} onKeyDown={onKeyDown}>
      <ComposeChecksSection
        draft={draft}
        accounts={accounts}
        known={known}
        onAttach={onAttach}
        onReplaceRecipient={onReplaceRecipient}
        onSwitchAccount={onSwitchAccount}
        onMoveToBcc={onMoveToBcc}
      />
      <RecipientChips recipients={recipients} selectedEmail={email} onSelect={setPicked} />
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
