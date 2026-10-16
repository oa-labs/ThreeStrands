import { useEffect, useMemo, useState, type KeyboardEvent, type ReactNode } from "react";
import { mailClient } from "./data/client";
import type { Account, ContactProfile, ContactTimelineItem, ThreadDetail } from "./domain";
import { ContactFilesSection, DomainSection, RecentEmailsSection, ThreadOutlineSection, type RecentEmailItem } from "./ContextSections";
import { RecipientChips, RecipientSummary } from "./RecipientSections";
import { parseAddress, splitAddressList } from "./emailAddress";
import { logBackgroundFailure } from "./errors";
import { SenderDecisions, TrustSection, useSenderHistory, type SenderActions, type SenderTarget } from "./SenderSections";
import { bulkMessage, latestInbound } from "./senderSignals";
import { threadTextIndex } from "./threadTextIndex";

/** A reply in the open conversation, as the context panel shows it. */
export type ReplyContext = {
  /** The reply's complete addresses, To first, without the user's own. */
  recipients: { email: string; name: string | null }[];
  /** Checks before sending, shown first. */
  checks?: ReactNode;
  review?: ReactNode;
  /** Open times to insert into the reply, below the recipient card. */
  availability?: ReactNode;
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
 * meetings, and an outline of a long conversation. Then what is about the
 * selected participant: files they sent, other emails with them, and other
 * people at their organization. Their contact card lives in the reader, on
 * hover over their name. Sections without content are left out.
 *
 * The selected participant is the latest external sender unless the reader
 * picked someone else by clicking their name in a message header.
 *
 * While the user replies in this conversation, the panel is about who the
 * reply goes to: the checks before sending, the reply's recipients (with
 * chips to choose among several), a card for the selected one, and open
 * times to offer come first, and the person sections follow that recipient.
 * The conversation sections stay below the card.
 *
 * Bulk mail (a List-Unsubscribe option, or the Updates or Promotions
 * category) adds a Trust section to the conversation group and puts the
 * sender's history and one-click decisions first in the person group. Other
 * conversations show Trust only when something needs a look.
 */
export function ContextPanel({ detail, accounts, selectedEmail = null, reply = null, onOpenThread, onShowMessage, assist, related, chat, sender, idle = null, onKeyDown }: {
  detail: ThreadDetail | null;
  accounts: Account[];
  /** A participant the reader picked from a message header; ignored if not on the conversation. */
  selectedEmail?: string | null;
  /**
   * The reply being written in this conversation: its recipients, To first,
   * and the sections to show above the conversation's own while it is open.
   */
  reply?: ReplyContext | null;
  onOpenThread(id: string): void;
  /** Reveals a message: in the reader when it belongs to the open conversation, otherwise by opening its conversation. */
  onShowMessage?(threadId: string, messageId: string): void;
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
    for (const message of [...detail.messages].reverse()) {
      const sender = parseAddress(message.sender);
      if (!own.has(sender.email.toLocaleLowerCase())) return sender.email.toLocaleLowerCase();
    }
    return participants[0]?.email ?? "";
  }, [detail, participants, own]);
  const picked = selectedEmail?.toLocaleLowerCase() ?? "";
  // A recipient chosen from the chips, until the reader picks someone again.
  const [chipPick, setChipPick] = useState<string | null>(null);
  const [pickSource, setPickSource] = useState(picked);
  if (pickSource !== picked) { setPickSource(picked); setChipPick(null); }
  const recipients = reply?.recipients ?? [];
  const isRecipient = (address: string | null) => Boolean(address) && recipients.some((item) => item.email === address);
  // While replying, only the reply's recipients can be selected; a pick from
  // the reader counts when it names one of them.
  const email = reply
    ? (isRecipient(chipPick) ? chipPick! : isRecipient(picked) ? picked : recipients[0]?.email ?? "")
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
  const showMessage = onShowMessage ?? ((threadId: string) => onOpenThread(threadId));
  return (
    <aside className="context-panel" aria-label="Conversation context" tabIndex={-1} onKeyDown={onKeyDown}>
      {detail && reply ? <>
        {reply.checks}
        {reply.review}
        <RecipientChips recipients={recipients} selectedEmail={email} onSelect={setChipPick} />
        {recipient ? (
          <RecipientSummary
            email={recipient.email}
            name={recipient.name}
            profile={loadedEmail === email ? profile : null}
          />
        ) : <p className="context-status compose-context-empty">Add a recipient to see your history with them.</p>}
        {reply.availability}
      </> : null}
      {detail ? null : idle}
      {detail ? assist : null}
      {detail && related ? related(person, [...meetingPeople.values()]) : null}
      {inbound ? <TrustSection key={inbound.id} message={inbound} bulk={Boolean(bulk)} prior={inboundPrior} /> : null}
      {detail ? <ThreadOutlineSection detail={detail} accounts={accounts} onShowMessage={showMessage} /> : null}
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
