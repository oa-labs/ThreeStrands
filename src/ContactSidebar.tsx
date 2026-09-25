import { useEffect, useMemo, useState } from "react";
import { BookUser, Heart, Mail } from "lucide-react";
import { mailClient } from "./data/client";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { Account, ContactProfile, ContactTimelineItem, ThreadDetail } from "./domain";
import { parseAddress, splitAddressList } from "./emailAddress";

export function ContactSidebar({ detail, accounts, onOpenThread }: {
  detail: ThreadDetail | null;
  accounts: Account[];
  onOpenThread(id: string): void;
}) {
  const own = new Set(accounts.map((account) => account.email.toLocaleLowerCase()));
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
  }, [detail, accounts]);
  const preferred = useMemo(() => {
    if (!detail) return "";
    for (const message of [...detail.messages].reverse()) {
      const sender = parseAddress(message.sender);
      if (!own.has(sender.email.toLocaleLowerCase())) return sender.email.toLocaleLowerCase();
    }
    return participants[0]?.email ?? "";
  }, [detail, participants, accounts]);
  const [email, setEmail] = useState("");
  const [profile, setProfile] = useState<ContactProfile | null>(null);
  const [timeline, setTimeline] = useState<ContactTimelineItem[]>([]);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => { setEmail(preferred); }, [preferred, detail?.thread.id]);
  useEffect(() => {
    if (!email) { setProfile(null); setTimeline([]); return; }
    let active = true;
    void (async () => {
      try {
        const found = await mailClient.listContactProfiles(email, 100);
        const match = found.find((contact) => contact.addresses.some((address) => address.toLocaleLowerCase() === email));
        const loaded = match ? await mailClient.getContactProfile(match.id) : null;
        const events = await mailClient.contactTimeline(loaded?.id ?? `derived:${email}`, 0, 5);
        if (active) { setProfile(loaded); setTimeline(events); setError(null); }
      } catch (reason) {
        if (active) setError(reason instanceof Error ? reason.message : String(reason));
      }
    })();
    return () => { active = false; };
  }, [email]);

  const selected = participants.find((item) => item.email === email);
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

  return (
    <aside className="contact-sidebar" aria-label="Contact details">
      <header><strong>Contact</strong></header>
      {participants.length > 1 ? (
        <label className="contact-participant-picker">Conversation participant
          <select aria-label="Conversation participant" value={email} onChange={(event) => setEmail(event.target.value)}>
            {participants.map((item) => <option key={item.email} value={item.email}>{item.name}</option>)}
          </select>
        </label>
      ) : null}
      {email ? (
        <>
          <div className="contact-sidebar-identity">
            <div className="contact-avatar medium">
              {profile?.photoData ? <img src={`data:image/jpeg;base64,${profile.photoData}`} alt="" /> : <span>{(profile?.displayName || selected?.name || email).slice(0, 1).toLocaleUpperCase()}</span>}
            </div>
            <h2>{profile?.displayName || selected?.name || email}</h2>
            <a href={`mailto:${email}`}><Mail size={14} />{email}</a>
            {profile?.role || profile?.company ? <p>{[profile.role, profile.company].filter(Boolean).join(" · ")}</p> : null}
            {profile?.location ? <p>{profile.location}</p> : null}
          </div>
          {profile ? (
            <>
              {profile.bio ? <p className="contact-sidebar-bio">{profile.bio}</p> : null}
              {profile.links.length ? <nav className="contact-sidebar-links" aria-label="Contact links">{profile.links.map((link) => <button type="button" key={link} onClick={() => void openUrl(link)}>{new URL(link).hostname}</button>)}</nav> : null}
              {profile.notes ? <section className="contact-sidebar-notes"><h3>Notes</h3><p>{profile.notes}</p></section> : null}
              <section className="contact-sidebar-history">
                <h3>Recent emails</h3>
                {timeline.map((item) => (
                  <button type="button" key={item.threadId} onClick={() => onOpenThread(item.threadId)}>
                    <strong>{item.subject || "(no subject)"}</strong>
                    <small>{new Date(item.sentAt).toLocaleDateString()} · {item.accountId}</small>
                  </button>
                ))}
              </section>
              <button className="contact-sidebar-favorite" type="button" onClick={() => void toggleFavorite()}>
                <Heart size={15} fill={profile.favorite ? "currentColor" : "none"} />
                {profile.favorite ? "Remove favorite" : "Add favorite"}
              </button>
            </>
          ) : (
            <button type="button" className="contact-sidebar-save" onClick={() => void save()}><BookUser size={15} />Save to contacts</button>
          )}
        </>
      ) : <p className="contacts-status">Select a conversation participant.</p>}
      {error ? <p className="contacts-error" role="alert">{error}</p> : null}
    </aside>
  );
}
