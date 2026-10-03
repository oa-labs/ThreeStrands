import { useEffect, useId, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { Check, Copy, Heart, Mail, UserPlus } from "lucide-react";
import { mailClient } from "./data/client";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { Account, ContactActivity, ContactProfile, ContactTimelineItem, ThreadDetail } from "./domain";
import { describeActivity } from "./contactContext";
import { ContactFilesSection, DomainSection, RecentEmailsSection, ThreadOutlineSection } from "./ContextSections";
import { parseAddress, splitAddressList } from "./emailAddress";
import { logBackgroundFailure } from "./errors";

/** The selected participant, once their contact record has been looked up. */
export type ContextPerson = {
  /** A saved contact id, or `derived:<email>` for someone not yet saved. */
  contactId: string;
  email: string;
  addresses: string[];
};

/**
 * The single right-side panel for a conversation: a compact card for the
 * selected participant with a line of history facts, then the AI brief and
 * suggestions, related tasks and meetings, files the person sent, an outline
 * of a long conversation, other emails with the person, and other people at
 * their organization. Sections without content are left out.
 */
export function ContextPanel({ detail, accounts, onOpenThread, onOpenContact, onShowMessage, assist, related, chat }: {
  detail: ThreadDetail | null;
  accounts: Account[];
  onOpenThread(id: string): void;
  onOpenContact(id: string): void;
  /** Reveals a message: in the reader when it belongs to the open conversation, otherwise by opening its conversation. */
  onShowMessage?(threadId: string, messageId: string): void;
  assist?: ReactNode;
  /** Sections about the conversation and the selected person. */
  related?(person: ContextPerson | null, meetingPeople: { email: string; name: string }[]): ReactNode;
  /** The question box, kept at the bottom of the panel. */
  chat?(person: ContextPerson | null): ReactNode;
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
  const [email, setEmail] = useState("");
  const [profile, setProfile] = useState<ContactProfile | null>(null);
  const [timeline, setTimeline] = useState<ContactTimelineItem[]>([]);
  const [loadedEmail, setLoadedEmail] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [emailCopied, setEmailCopied] = useState(false);
  const [emailCopyFailed, setEmailCopyFailed] = useState(false);
  const openHintId = useId();
  const participantCountId = useId();
  const participantListRef = useRef<HTMLDivElement>(null);

  useEffect(() => { setEmail(preferred); setEmailCopied(false); setEmailCopyFailed(false); }, [preferred, detail?.thread.id]);
  useEffect(() => { setEmailCopied(false); setEmailCopyFailed(false); }, [email]);
  useEffect(() => {
    if (!email) { setProfile(null); setTimeline([]); setLoadedEmail(""); return; }
    let active = true;
    void (async () => {
      try {
        const owners = await mailClient.resolveContactIds([email]);
        const loaded = await mailClient.getContactProfile(owners[email] ?? `derived:${email}`);
        const events = await mailClient.contactTimeline(loaded?.id ?? `derived:${email}`, 0, 6);
        if (active) { setProfile(loaded); setTimeline(events); setLoadedEmail(email); setError(null); }
      } catch (reason) {
        if (active) setError(reason instanceof Error ? reason.message : String(reason));
      }
    })();
    return () => { active = false; };
  }, [email]);

  // One chip per person: addresses linked to the same saved contact share a
  // chip, and anyone without a saved contact keeps a chip per address.
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

  useLayoutEffect(() => {
    const list = participantListRef.current;
    if (!list) return;
    const revealSelected = () => {
      const button = list.querySelector<HTMLButtonElement>('button[aria-pressed="true"]');
      if (button) revealParticipant(list, button);
    };
    revealSelected();
    // Names, panel width, and font preferences can change the badge wrapping.
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(revealSelected);
    observer.observe(list);
    return () => observer.disconnect();
  }, [chips, email, detail?.thread.id]);

  const selected = participants.find((item) => item.email === email);
  const displayName = profile?.displayName || selected?.name || email;
  const person = useMemo<ContextPerson | null>(() => {
    if (!email || loadedEmail !== email) return null;
    return profile
      ? { contactId: profile.id, email, addresses: profile.addresses }
      : { contactId: `derived:${email}`, email, addresses: [email] };
  }, [email, loadedEmail, profile]);
  const meetingPeople = new Map(chips.flatMap((chip) => chip.emails.map((address) =>
    [address.toLocaleLowerCase(), { email: address, name: chip.name || address }] as const)));
  if (person && profile) {
    for (const address of profile.addresses) {
      meetingPeople.set(address.toLocaleLowerCase(), { email: address, name: displayName });
    }
  }
  const [activity, setActivity] = useState<{ contactId: string; value: ContactActivity } | null>(null);
  const personId = person?.contactId ?? null;
  useEffect(() => {
    if (!personId) return;
    let active = true;
    mailClient.contactActivity(personId)
      .then((value) => { if (active) setActivity({ contactId: personId, value }); })
      .catch(logBackgroundFailure("Contact activity lookup"));
    return () => { active = false; };
  }, [personId]);
  const currentActivity = activity && activity.contactId === personId ? activity.value : null;
  const facts = currentActivity ? describeActivity(currentActivity) : [];
  // Local history confirms every email with the person is in this conversation.
  const everythingHere = Boolean(currentActivity && currentActivity.threadCount === 1
    && timeline.some((item) => item.threadId === detail?.thread.id));
  const otherEmails = timeline.filter((item) => item.threadId !== detail?.thread.id).slice(0, 5);
  const showMessage = onShowMessage ?? ((threadId: string) => onOpenThread(threadId));
  const save = async () => {
    try {
      const saved = await mailClient.saveContactProfile({
        id: null, displayName: selected?.name ?? null, role: null, company: null,
        location: null, bio: null, notes: null, links: [], photoData: null,
        favorite: false, addresses: [email],
      });
      setProfile(saved);
    } catch (reason) { setError(reason instanceof Error ? reason.message : String(reason)); }
  };
  const toggleFavorite = async () => {
    if (!profile) return;
    setError(null);
    try {
      const updated = await mailClient.saveContactProfile({ ...profile, favorite: !profile.favorite });
      setProfile(updated);
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    }
  };
  const copyEmail = async () => {
    setEmailCopyFailed(false);
    try {
      await navigator.clipboard.writeText(email);
      setEmailCopied(true);
    } catch {
      setEmailCopyFailed(true);
    }
  };

  return (
    <aside className="context-panel" aria-label="Conversation context">
      {chips.length > 1 ? (
        <div className="context-participants-section">
          <div id={participantCountId} className="context-participants-label">Participants · {chips.length}</div>
          <div ref={participantListRef} className="context-participants" role="group" aria-label="Conversation participants" aria-describedby={participantCountId}
            onFocusCapture={(event) => {
              if (event.target instanceof HTMLButtonElement) revealParticipant(event.currentTarget, event.target);
            }}>
            {chips.map((chip) => (
              <button key={chip.key} type="button" aria-pressed={chip.emails.includes(email)} title={chip.emails.join(", ")} onClick={() => { if (!chip.emails.includes(email)) setEmail(chip.emails[0]); }}>
                <span className="context-participant-initial" aria-hidden="true">{(chip.name || chip.emails[0]).slice(0, 1).toLocaleUpperCase()}</span>
                {chip.name ? <span>{chip.name}</span> : <ParticipantAddress email={chip.emails[0]} />}
              </button>
            ))}
          </div>
        </div>
      ) : null}
      {email ? (
        <section className="context-contact" aria-label="Contact">
          <div className="context-contact-card">
            <div className="contact-avatar small">
              {profile?.photoData ? <img src={`data:image/jpeg;base64,${profile.photoData}`} alt="" /> : <span>{displayName.slice(0, 1).toLocaleUpperCase()}</span>}
            </div>
            <div className="context-contact-text">
              <div className="context-contact-name-row">
                <h2>{profile
                  ? <button type="button" className="context-contact-name" aria-describedby={openHintId} onClick={() => onOpenContact(profile.id)}>{displayName}</button>
                  : displayName}</h2>
                {profile ? (
                  <button type="button" className="context-icon-button context-contact-favorite" aria-label={profile.favorite ? "Remove favorite" : "Add favorite"} aria-pressed={profile.favorite} title={profile.favorite ? "Remove favorite" : "Add favorite"} onClick={() => void toggleFavorite()}>
                    <Heart size={15} fill={profile.favorite ? "currentColor" : "none"} />
                  </button>
                ) : (
                  <button type="button" className="context-icon-button" aria-label="Save to contacts" title="Save to contacts" onClick={() => void save()}>
                    <UserPlus size={15} />
                  </button>
                )}
                <span id={openHintId} hidden>Opens in Contacts</span>
              </div>
              <div className="contact-sidebar-email-row">
                <a href={`mailto:${email}`}><Mail size={13} /><span>{email}</span></a>
                <button type="button" className="contact-sidebar-email-copy" aria-label={emailCopied ? "Copied email address" : "Copy email address"} onClick={() => void copyEmail()}>
                  {emailCopied ? <Check size={13} /> : <Copy size={13} />}
                </button>
              </div>
              {emailCopyFailed ? <span className="contact-sidebar-copy-status" role="status">Could not copy email address</span> : null}
              {facts.length > 0 ? <p className="context-contact-activity">{facts.join(" · ")}</p> : null}
              {profile?.role || profile?.company ? <p>{[profile.role, profile.company].filter(Boolean).join(" · ")}</p> : null}
              {profile?.location ? <p>{profile.location}</p> : null}
              {profile?.links.length ? <nav className="contact-sidebar-links" aria-label="Contact links">{profile.links.map((link) => <a key={link} href={link} onClick={(event) => { event.preventDefault(); void openUrl(link); }}>{new URL(link).hostname}</a>)}</nav> : null}
            </div>
          </div>
          {profile?.bio ? <p className="contact-sidebar-bio">{profile.bio}</p> : null}
          {profile?.notes ? <section className="contact-sidebar-notes"><h3>Notes</h3><p>{profile.notes}</p></section> : null}
        </section>
      ) : <p className="contacts-status">Select a conversation participant.</p>}
      {detail ? assist : null}
      {detail && related ? related(person, [...meetingPeople.values()]) : null}
      {person ? <ContactFilesSection key={person.contactId} contactId={person.contactId} name={displayName} onShowMessage={showMessage} /> : null}
      {detail ? <ThreadOutlineSection detail={detail} accounts={accounts} onShowMessage={showMessage} /> : null}
      {person && (otherEmails.length > 0 || everythingHere) ? (
        <RecentEmailsSection items={otherEmails} name={displayName} onOpenThread={onOpenThread} />
      ) : null}
      {person ? (
        <DomainSection
          email={person.email}
          addresses={person.addresses}
          accounts={accounts}
          hideThreadIds={[...(detail ? [detail.thread.id] : []), ...otherEmails.map((item) => item.threadId)]}
          onOpenThread={onOpenThread}
        />
      ) : null}
      {error ? <p className="contacts-error" role="alert">{error}</p> : null}
      {detail && chat ? <div className="context-chat-dock">{chat(person)}</div> : null}
    </aside>
  );
}

/** Reveal a badge without scrolling the contact card or the surrounding app. */
function revealParticipant(list: HTMLElement, button: HTMLButtonElement) {
  const viewport = list.getBoundingClientRect();
  if (!viewport.height) return;
  const badge = button.getBoundingClientRect();
  if (badge.top < viewport.top) list.scrollTop += badge.top - viewport.top;
  else if (badge.bottom > viewport.bottom) list.scrollTop += badge.bottom - viewport.bottom;
}

function ParticipantAddress({ email }: { email: string }) {
  const at = email.lastIndexOf("@");
  if (at <= 0) return <span>{email}</span>;
  return (
    <span className="context-participant-address">
      <span className="context-participant-local">{email.slice(0, at)}</span>@<span className="context-participant-domain">{email.slice(at + 1)}</span>
    </span>
  );
}
