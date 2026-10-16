import { useEffect, useMemo, useState, type KeyboardEvent, type ReactNode } from "react";
import { mailClient } from "./data/client";
import type { Account, ContactProfile, ContactTimelineItem, ThreadDetail } from "./domain";
import { ContactFilesSection, DomainSection, RecentEmailsSection, ThreadOutlineSection, type RecentEmailItem } from "./ContextSections";
import { RecipientSummary } from "./RecipientSections";
import { ContextPersonHeader, type ContextPersonOption } from "./ContextPersonHeader";
import { isColleagueAddress } from "./contactContext";
import { messagePerson } from "./readingMessage";
import { parseAddress, splitAddressList } from "./emailAddress";
import { logBackgroundFailure } from "./errors";
import { SenderDecisions, TrustSection, useSenderHistory, type SenderActions, type SenderTarget } from "./SenderSections";
import { bulkMessage, latestInbound, type TrustLink } from "./senderSignals";
import { threadTextIndex } from "./threadTextIndex";

/** A reply in the open conversation, as the context panel shows it. */
export type ReplyContext = {
  /** The reply's complete addresses, To first, without the user's own. */
  recipients: { email: string; name: string | null }[];
  /** Checks before sending, shown first. */
  checks?: ReactNode;
};

/** The selected participant, once their contact record has been looked up. */
export type ContextPerson = {
  /** A saved contact id, or `derived:<email>` for someone not yet saved. */
  contactId: string;
  email: string;
  addresses: string[];
};

/**
 * The single right-side panel for a conversation, in two groups. First what
 * is about the conversation: the AI brief and suggestions, related tasks and
 * meetings, and an outline of a long conversation. Then, under a header
 * naming them, what is about the selected participant: files they sent, other
 * emails with them, and other people at their organization. Their contact
 * card lives in the reader, on hover over their name. Sections without
 * content are left out.
 *
 * The selected participant is the person of the message being read: its
 * sender, or whom the user wrote to. Picking someone, by their name in a
 * message header or from the header's switcher, holds until the reader moves
 * to another message. The switcher lists the conversation's correspondents
 * before the user's colleagues.
 *
 * The AI actions, Find Times and the brief, share one row at the top: each is
 * a button until it has something to show, then opens to full width below
 * the buttons. Find Times is left out while reading bulk mail, but offered
 * when replying to it.
 *
 * While the user replies in this conversation, the checks before sending
 * come first, and the person group is about who the reply
 * goes to: only its recipients can be selected, and a card shows what is
 * saved about the selected one.
 *
 * Bulk mail (a List-Unsubscribe option, or the Updates or Promotions
 * category) adds a Trust section to the conversation group and puts the
 * sender's history and one-click decisions first in the person group. Other
 * conversations show Trust only when something needs a look.
 */
export function ContextPanel({ detail, accounts, selectedEmail = null, readingMessageId = null, onSelectPerson, reply = null, onOpenThread, onShowMessage, onShowLink, availability, assist, related, chat, sender, idle = null, onKeyDown }: {
  detail: ThreadDetail | null;
  accounts: Account[];
  /** A participant the reader picked from a message header; ignored if not on the conversation. */
  selectedEmail?: string | null;
  /** The message being read, whose person the panel shows unless someone is picked. */
  readingMessageId?: string | null;
  /** Called when the reader picks someone in the panel's person header. */
  onSelectPerson?(email: string): void;
  /**
   * The reply being written in this conversation: its recipients, To first,
   * and the sections to show above the conversation's own while it is open.
   */
  reply?: ReplyContext | null;
  onOpenThread(id: string): void;
  /** Reveals a message: in the reader when it belongs to the open conversation, otherwise by opening its conversation. */
  onShowMessage?(threadId: string, messageId: string): void;
  /** Shows a link the Trust section warns about in the open message. */
  onShowLink?(threadId: string, messageId: string, link: TrustLink): void;
  /** Open times from the calendar to offer in a reply. */
  availability?: ReactNode;
  assist?: ReactNode;
  /** Sections about the conversation and the selected person. */
  related?(person: ContextPerson | null, meetingPeople: { email: string; name: string }[]): ReactNode;
  /** The question box, kept at the bottom of the panel. */
  chat?(person: ContextPerson | null): ReactNode;
  /** Decisions about a bulk sender; without them the Sender section is left out. */
  sender?: SenderActions;
  /** Keyboard handling for the panel while focus is inside it. */
  onKeyDown?(event: KeyboardEvent<HTMLElement>): void;
  /** Shown when no conversation is open, such as what's next at inbox zero. */
  idle?: ReactNode;
}) {
  const own = useMemo(() => new Set(accounts.map((account) => account.email.toLocaleLowerCase())), [accounts]);
  const participants = useMemo(() => {
    if (!detail) return [];
    const byEmail = new Map<string, string>();
    for (const message of detail.messages) {
      for (const raw of [message.sender, ...message.recipients.flatMap(splitAddressList)]) {
        const address = parseAddress(raw);
        if (address.email.includes("@") && !own.has(address.email.toLocaleLowerCase())) {
          byEmail.set(address.email.toLocaleLowerCase(), address.name);
        }
      }
    }
    return [...byEmail].map(([email, name]) => ({ email, name }));
  }, [detail, own]);
  const preferred = useMemo(() => {
    if (!detail) return "";
    const reading = detail.messages.find((message) => message.id === readingMessageId);
    const readingPerson = reading ? messagePerson(reading, own) : null;
    if (readingPerson) return readingPerson;
    for (const message of [...detail.messages].reverse()) {
      const sender = parseAddress(message.sender);
      if (!own.has(sender.email.toLocaleLowerCase())) return sender.email.toLocaleLowerCase();
    }
    return participants[0]?.email ?? "";
  }, [detail, participants, own, readingMessageId]);
  // A person chosen in the header, until the reader picks someone else in a
  // message or moves to another message.
  const [headerPick, setHeaderPick] = useState<string | null>(null);
  const pickSource = `${selectedEmail?.toLocaleLowerCase() ?? ""}\n${readingMessageId ?? ""}`;
  const [lastPickSource, setLastPickSource] = useState(pickSource);
  if (lastPickSource !== pickSource) { setLastPickSource(pickSource); setHeaderPick(null); }
  const picked = headerPick ?? selectedEmail?.toLocaleLowerCase() ?? "";
  const selectPerson = (address: string) => {
    setHeaderPick(address);
    onSelectPerson?.(address);
  };
  const ownEmails = useMemo(() => accounts.map((account) => account.email), [accounts]);
  const replyRecipients = reply?.recipients;
  const recipients = useMemo(() => replyRecipients ?? [], [replyRecipients]);
  const firstRecipient = recipients.find((item) => !isColleagueAddress(item.email, ownEmails)) ?? recipients[0];
  const isRecipient = (address: string | null) => Boolean(address) && recipients.some((item) => item.email === address);
  // While replying, only the reply's recipients can be selected; a pick, or
  // the person of the message being read, counts when it names one of them.
  // Otherwise the first recipient who is not the user's colleague.
  const email = reply
    ? (isRecipient(picked) ? picked : isRecipient(preferred) ? preferred : firstRecipient?.email ?? "")
    : picked && participants.some((item) => item.email === picked) ? picked : preferred;
  const [profile, setProfile] = useState<ContactProfile | null>(null);
  const [timeline, setTimeline] = useState<ContactTimelineItem[]>([]);
  const [loadedEmail, setLoadedEmail] = useState("");
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    if (!email) { setProfile(null); setTimeline([]); setLoadedEmail(""); return; }
    let active = true;
    void (async () => {
      try {
        const owners = await mailClient.resolveContactIds([email]);
        const loaded = await mailClient.getContactProfile(owners[email] ?? `derived:${email}`);
        const contactId = loaded?.id ?? `derived:${email}`;
        const events = await mailClient.contactTimeline(contactId, 0, 6);
        if (active) { setProfile(loaded); setTimeline(events); setLoadedEmail(email); setError(null); }
      } catch (reason) {
        if (active) setError(reason instanceof Error ? reason.message : String(reason));
      }
    })();
    return () => { active = false; };
  }, [email]);

  // One entry per person, for naming meeting attendees: addresses linked to
  // the same saved contact share a name, and anyone else keeps their own.
  const participantKey = participants.map((item) => item.email).join("\n");
  const [participantContacts, setParticipantContacts] = useState<{
    owners: Record<string, string>;
    names: Record<string, string>;
  }>({ owners: {}, names: {} });
  // Re-resolve after the selected contact is saved or its addresses change.
  const profileKey = profile ? `${profile.id}\n${profile.addresses.join("\n")}` : "";
  useEffect(() => {
    const emails = participantKey ? participantKey.split("\n") : [];
    if (emails.length < 2) { setParticipantContacts({ owners: {}, names: {} }); return; }
    let active = true;
    void (async () => {
      try {
        const resolved = await mailClient.resolveContactIds(emails);
        const targets = new Map<string, string>();
        for (const email of emails) {
          const owner = resolved[email];
          targets.set(owner ?? `address:${email}`, owner ?? `derived:${email}`);
        }
        const entries = [...targets];
        const profiles = await Promise.all(entries.map(([, id]) => mailClient.getContactProfile(id).catch((reason) => {
          logBackgroundFailure("Participant contact name lookup")(reason);
          return null;
        })));
        if (active) setParticipantContacts({
          owners: resolved,
          names: Object.fromEntries(profiles.flatMap((item, index) =>
            item?.id === entries[index][1] && item.displayName ? [[entries[index][0], item.displayName]] : [])),
        });
      } catch (reason) {
        if (active) setParticipantContacts({ owners: {}, names: {} });
        logBackgroundFailure("Participant contact lookup")(reason);
      }
    })();
    return () => { active = false; };
  }, [participantKey, profileKey]);
  const chips = useMemo(() => {
    const byOwner = new Map<string, { emails: string[]; name: string }>();
    for (const item of participants) {
      const key = participantContacts.owners[item.email] ?? `address:${item.email}`;
      const named = participantContacts.names[key] || (item.name !== item.email ? item.name : "");
      const existing = byOwner.get(key);
      if (existing) {
        existing.emails.push(item.email);
        if (!existing.name) existing.name = named;
      } else {
        byOwner.set(key, { emails: [item.email], name: named });
      }
    }
    return [...byOwner].map(([key, chip]) => ({ key, ...chip }));
  }, [participants, participantContacts]);

  const selected = participants.find((item) => item.email === email);
  const recipient = recipients.find((item) => item.email === email) ?? null;
  const displayName = profile?.displayName || selected?.name || recipient?.name || email;
  const person = useMemo<ContextPerson | null>(() => {
    if (!email || loadedEmail !== email) return null;
    return profile
      ? { contactId: profile.id, email, addresses: profile.addresses }
      : { contactId: `derived:${email}`, email, addresses: [email] };
  }, [email, loadedEmail, profile]);
  const meetingPeople = new Map(chips.flatMap((chip) => chip.emails.map((address) =>
    [address.toLocaleLowerCase(), { email: address, name: chip.name || address }] as const)));
  for (const item of recipients) {
    if (!meetingPeople.has(item.email)) meetingPeople.set(item.email, { email: item.email, name: item.name || item.email });
  }
  if (person && profile) {
    for (const address of profile.addresses) {
      meetingPeople.set(address.toLocaleLowerCase(), { email: address, name: displayName });
    }
  }
  const inbound = useMemo(() => detail ? latestInbound(detail, own) : null, [detail, own]);
  // The conversation text before the inbound message, so Trust folds its quoted history as the reader does.
  const inboundPrior = useMemo(() => {
    if (!detail || !inbound?.bodyHtml.trim()) return undefined;
    return threadTextIndex(detail.messages).before(detail.messages.indexOf(inbound));
  }, [detail, inbound]);
  const bulk = useMemo(() => detail ? bulkMessage(detail, own) : null, [detail, own]);
  const senderTarget = useMemo<SenderTarget | null>(() => {
    if (!detail || !sender || !bulk) return null;
    const from = parseAddress(bulk.sender);
    return from.email.toLocaleLowerCase() === email ? { accountId: detail.thread.accountId, email, name: from.name } : null;
  }, [detail, sender, bulk, email]);
  const senderHistory = useSenderHistory(senderTarget, detail?.thread.id ?? "");
  // The contact index leaves automated mail out, so a bulk sender's own mail
  // joins the person's conversations in one newest-first list.
  const recentEmails = useMemo<RecentEmailItem[]>(() => {
    const otherEmails = timeline.filter((item) => item.threadId !== detail?.thread.id).slice(0, 5);
    const merged = new Map<string, RecentEmailItem>(otherEmails.map((item) => [item.threadId, item]));
    for (const item of senderHistory.history?.threads ?? []) {
      if (item.threadId !== detail?.thread.id && !merged.has(item.threadId)) merged.set(item.threadId, item);
    }
    return [...merged.values()].sort((left, right) => right.sentAt.localeCompare(left.sentAt));
  }, [timeline, senderHistory.history, detail?.thread.id]);
  // Whom the header can switch to: the reply's recipients while replying,
  // otherwise everyone on the conversation, one entry per saved contact. The
  // user's colleagues come last, since the conversation is rarely about them.
  const switchable = useMemo<ContextPersonOption[]>(() => {
    const options: ContextPersonOption[] = reply
      ? recipients.map((item) => ({
        email: item.email,
        name: item.name || chips.find((chip) => chip.emails.includes(item.email))?.name || null,
      }))
      : chips.map((chip) => ({ email: chip.emails[0], emails: chip.emails, name: chip.name || null }));
    const colleague = (option: ContextPersonOption) => isColleagueAddress(option.email, ownEmails);
    return [...options.filter((option) => !colleague(option)), ...options.filter(colleague)];
  }, [reply, recipients, chips, ownEmails]);
  const showMessage = onShowMessage ?? ((threadId: string) => onOpenThread(threadId));
  return (
    <aside className="context-panel" aria-label="Conversation context" tabIndex={-1} onKeyDown={onKeyDown}>
      {detail && reply ? reply.checks : null}
      {detail ? null : idle}
      {detail ? <div className="context-ai-row">{bulk && !reply ? null : availability}{assist}</div> : null}
      {detail && related ? related(person, [...meetingPeople.values()]) : null}
      {inbound ? <TrustSection key={inbound.id} message={inbound} bulk={Boolean(bulk)} prior={inboundPrior}
        onShowLink={onShowLink && detail ? (link) => onShowLink(detail.thread.id, inbound.id, link) : undefined} /> : null}
      {detail ? <ThreadOutlineSection detail={detail} accounts={accounts} onShowMessage={showMessage} /> : null}
      {detail ? (
        <ContextPersonHeader
          people={switchable}
          selectedEmail={email}
          displayName={loadedEmail === email ? profile?.displayName : null}
          onSelect={selectPerson}
        />
      ) : null}
      {detail && reply ? (recipient ? (
        <RecipientSummary
          email={recipient.email}
          name={recipient.name}
          profile={loadedEmail === email ? profile : null}
        />
      ) : <p className="context-status compose-context-empty">Add a recipient to see your history with them.</p>) : null}
      {person ? <ContactFilesSection key={person.contactId} contactId={person.contactId} onShowMessage={showMessage} /> : null}
      {senderTarget && sender ? (
        // Waits for the sender's mail so the rows arrive together.
        senderHistory.settled ? (
          <RecentEmailsSection
            items={recentEmails}
            onOpenThread={onOpenThread}
            closing={<SenderDecisions key={detail?.thread.id} target={senderTarget} history={senderHistory.history} actions={sender} />}
          />
        ) : null
      ) : person && recentEmails.length > 0 ? <RecentEmailsSection items={recentEmails} onOpenThread={onOpenThread} /> : null}
      {person ? (
        <DomainSection
          email={person.email}
          addresses={person.addresses}
          accounts={accounts}
          hideThreadIds={[...(detail ? [detail.thread.id] : []), ...recentEmails.map((item) => item.threadId)]}
          onOpenThread={onOpenThread}
        />
      ) : null}
      {error ? <p className="contacts-error" role="alert">{error}</p> : null}
      {detail && chat ? <div className="context-chat-dock">{chat(person)}</div> : null}
    </aside>
  );
}
