import { useEffect, useRef, useState } from "react";
import type { Account } from "./domain";
import {
  WRITING_PROFILE_LIMITS,
  setWritingProfile,
  useWritingProfiles,
  writingProfileFor,
  type WritingProfile,
} from "./writingProfiles";

/**
 * Who the user is and how they want drafts reviewed, per sending account, so
 * Draft Review does not need it explained in every email's goal. Edits save
 * when a field loses focus or the section closes. The profile stays editable
 * while Draft Review is off, so what was written never seems to disappear.
 */
export function WritingProfileSettings({ accounts, draftReviewOn, onOpenAiSettings, onChange }: {
  accounts: Account[];
  /** Whether Draft Review can use the profile: a provider is chosen and Writing is on. */
  draftReviewOn: boolean;
  onOpenAiSettings?(): void;
  /** Queues the portable preference record; see `AiProviderSettings`. */
  onChange?: () => void;
}) {
  const profiles = useWritingProfiles();
  const [account, setAccount] = useState(() => accounts[0]?.email ?? "");
  // Accounts can load after Settings opens, or the chosen one be removed.
  const selected = accounts.some((entry) => entry.email === account) ? account : accounts[0]?.email ?? "";
  const [draft, setDraft] = useState<WritingProfile>(() => writingProfileFor(profiles, selected));
  const pending = useRef<{ account: string; profile: WritingProfile } | null>(null);
  const onChangeRef = useRef(onChange);
  onChangeRef.current = onChange;

  function commit() {
    const edit = pending.current;
    if (!edit) return;
    pending.current = null;
    setWritingProfile(edit.account, edit.profile);
    onChangeRef.current?.();
  }

  // Closing Settings mid-edit still keeps what was typed.
  useEffect(() => commit, []);

  // A profile saved on another device shows once this one is not being edited.
  useEffect(() => {
    if (!pending.current) setDraft(writingProfileFor(profiles, selected));
  }, [profiles, selected]);

  if (!selected) return <section className="settings-section writing-profile-settings" aria-label="Writing Profile">
    <p>Connect a mail account to write a profile for it.</p>
  </section>;

  function edit(field: keyof WritingProfile, value: string) {
    const profile = { ...draft, [field]: value };
    setDraft(profile);
    pending.current = { account: selected, profile };
  }

  function chooseAccount(next: string) {
    commit();
    setAccount(next);
  }

  return <section className="settings-section writing-profile-settings" aria-label="Writing Profile">
    <p>
      Draft Review sends this to your AI provider with every review of email you write from this account, along with
      any goal you add for that email. It treats what you write in About You as true and may suggest using it.
    </p>
    {draftReviewOn ? null : <p className="settings-hint" role="note">
      Draft Review is off, so this profile isn’t used yet. Turn on Writing in AI Provider to use it.
      {onOpenAiSettings ? <> <button type="button" className="btn-link" onClick={onOpenAiSettings}>AI Provider</button></> : null}
    </p>}
    {accounts.length > 1 ? <label className="settings-field">
      <span>Account</span>
      <select value={selected} onChange={(event) => chooseAccount(event.target.value)}>
        {accounts.map((entry) => <option key={entry.email} value={entry.email}>{entry.nickname ? `${entry.nickname} (${entry.email})` : entry.email}</option>)}
      </select>
    </label> : null}
    <label className="settings-field">
      <span>About You</span>
      <textarea
        rows={10}
        value={draft.aboutYou}
        maxLength={WRITING_PROFILE_LIMITS.aboutYou}
        placeholder="e.g. I’m the CEO of a 30-person consulting firm. I write to operations leaders about process improvement projects."
        onChange={(event) => edit("aboutYou", event.target.value)}
        onBlur={commit}
      />
    </label>
    <CharacterCount value={draft.aboutYou} limit={WRITING_PROFILE_LIMITS.aboutYou} />
    <label className="settings-field">
      <span>Review Instructions</span>
      <textarea
        rows={8}
        value={draft.reviewInstructions}
        maxLength={WRITING_PROFILE_LIMITS.reviewInstructions}
        placeholder="e.g. Help me move opportunities forward: one clear next step, no hedging, under 150 words."
        onChange={(event) => edit("reviewInstructions", event.target.value)}
        onBlur={commit}
      />
    </label>
    <CharacterCount value={draft.reviewInstructions} limit={WRITING_PROFILE_LIMITS.reviewInstructions} />
  </section>;
}

function CharacterCount({ value, limit }: { value: string; limit: number }) {
  return <small className="settings-hint writing-profile-count">{`${[...value].length.toLocaleString()} / ${limit.toLocaleString()}`}</small>;
}
