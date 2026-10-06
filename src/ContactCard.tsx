import { createContext, useEffect, useId, useState } from "react";
import { Check, Copy, Heart, Mail, UserPlus } from "lucide-react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { mailClient } from "./data/client";
import type { ContactProfile } from "./domain";
import { describeActivity } from "./contactContext";
import { describeDue, isKeepInTouchDue } from "./keepInTouch";
import { errorMessage } from "./errors";

/**
 * What a contact card in the reader needs from the app: opening the address
 * book, and making a person the subject of the context panel. Without a
 * provider, reader addresses fall back to a plain address-and-copy popover.
 */
export type ContactCardActions = {
  onOpenContact(id: string): void;
  onSelectPerson(email: string): void;
  /** The person the context panel is currently about, lowercased. */
  selectedEmail: string | null;
};

export const ContactCardContext = createContext<ContactCardActions | null>(null);

/**
 * A compact card for one person: avatar, name (opens the contact when saved),
 * favorite or save, email with copy, role and company, and a line of facts
 * from local history. The context panel and the reader's address hover card
 * both show it.
 */
export function ContactCard({ email, fallbackName, profile, loaded = true, facts, titleAs = "h2", showNotes = false, onOpenContact, onProfileSaved, onError }: {
  email: string;
  /** The name from the message, used until a saved contact supplies one. */
  fallbackName?: string;
  profile: ContactProfile | null;
  /** False while the contact lookup runs, so save and favorite don't flicker. */
  loaded?: boolean;
  facts: string[];
  /** The sidebar names the person with a heading; the reader's hover card does not. */
  titleAs?: "h2" | "div";
  showNotes?: boolean;
  onOpenContact(id: string): void;
  onProfileSaved(profile: ContactProfile): void;
  onError(message: string): void;
}) {
  const openHintId = useId();
  const [copied, setCopied] = useState(false);
  const [copyFailed, setCopyFailed] = useState(false);
  useEffect(() => { setCopied(false); setCopyFailed(false); }, [email]);
  const displayName = profile?.displayName || fallbackName || email;
  const Title = titleAs;

  const save = async () => {
    try {
      onProfileSaved(await mailClient.saveContactProfile({
        id: null, displayName: fallbackName ?? null, role: null, company: null,
        location: null, bio: null, notes: null, links: [], photoData: null,
        favorite: false, addresses: [email], birthday: null,
      }));
    } catch (reason) { onError(errorMessage(reason)); }
  };
  const toggleFavorite = async () => {
    if (!profile) return;
    try {
      onProfileSaved(await mailClient.saveContactProfile({ ...profile, favorite: !profile.favorite }));
    } catch (reason) { onError(errorMessage(reason)); }
  };
  const copyEmail = async () => {
    setCopyFailed(false);
    try {
      await navigator.clipboard.writeText(email);
      setCopied(true);
    } catch {
      setCopyFailed(true);
    }
  };

  return (
    <div className="context-contact-body">
      <div className="context-contact-card">
        <div className="contact-avatar small">
          {profile?.photoData ? <img src={`data:image/jpeg;base64,${profile.photoData}`} alt="" /> : <span>{displayName.slice(0, 1).toLocaleUpperCase()}</span>}
        </div>
        <div className="context-contact-text">
          <div className="context-contact-name-row">
            <Title className="context-contact-title">{profile
              ? <button type="button" className="context-contact-name" aria-describedby={openHintId} onClick={() => onOpenContact(profile.id)}>{displayName}</button>
              : displayName}</Title>
            {!loaded ? null : profile ? (
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
            <button type="button" className="contact-sidebar-email-copy" aria-label={copied ? "Copied email address" : "Copy email address"} onClick={() => void copyEmail()}>
              {copied ? <Check size={13} /> : <Copy size={13} />}
            </button>
          </div>
          {copyFailed ? <span className="contact-sidebar-copy-status" role="status">Could not copy email address</span> : null}
          {profile?.role || profile?.company ? <p>{[profile.role, profile.company].filter(Boolean).join(" · ")}</p> : null}
          {facts.length > 0 ? <p className="context-contact-activity">{facts.join(" · ")}</p> : null}
          {profile?.keepInTouchDueAt && isKeepInTouchDue(profile) ? <p className="context-contact-keep-in-touch">Keep in touch: {describeDue(profile.keepInTouchDueAt).toLocaleLowerCase()}</p> : null}
          {profile?.location ? <p>{profile.location}</p> : null}
          {profile?.links.length ? <nav className="contact-sidebar-links" aria-label="Contact links">{profile.links.map((link) => <a key={link} href={link} onClick={(event) => { event.preventDefault(); void openUrl(link); }}>{new URL(link).hostname}</a>)}</nav> : null}
        </div>
      </div>
      {showNotes && profile?.notes ? <section className="contact-sidebar-notes"><h3>Notes</h3><p>{profile.notes}</p></section> : null}
    </div>
  );
}

/**
 * Looks up one address's saved contact and history facts while `enabled`.
 * Each time it turns on it reads again, so a hover card reflects changes made
 * elsewhere, such as a favorite set in the context panel.
 */
export function useContactLookup(email: string, enabled: boolean) {
  const [state, setState] = useState<{ email: string; profile: ContactProfile | null; facts: string[] } | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    if (!enabled || !email) return;
    let active = true;
    setError(null);
    void (async () => {
      try {
        const owners = await mailClient.resolveContactIds([email]);
        const id = owners[email] ?? `derived:${email}`;
        const profile = await mailClient.getContactProfile(id);
        const activity = await mailClient.contactActivity(profile?.id ?? id);
        if (active) setState({ email, profile, facts: describeActivity(activity) });
      } catch (reason) {
        if (active) setError(errorMessage(reason));
      }
    })();
    return () => { active = false; };
  }, [email, enabled]);
  const current = state?.email === email ? state : null;
  return {
    loaded: Boolean(current),
    profile: current?.profile ?? null,
    facts: current?.facts ?? [],
    error,
    setError,
    setProfile: (profile: ContactProfile) => setState((previous) => ({ email, facts: previous?.email === email ? previous.facts : [], profile })),
  };
}
