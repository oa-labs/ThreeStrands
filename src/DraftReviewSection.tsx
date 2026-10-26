import { useEffect, useRef, useState } from "react";
import { Sparkles, X } from "lucide-react";
import { ContextSectionHeader } from "./ContextSections";
import { mailClient } from "./data/client";
import { readAiProvider, readAiRequestConfig } from "./aiSettings";
import type { DraftReviewResult } from "./domain";
import { reviewHasRevision, type DraftReviewActions, type DraftReviewSnapshot } from "./draftReview";
import { errorMessage } from "./errors";
import { ICON_SIZE } from "./iconSizes";
import { hasWritingProfile, useWritingProfiles, writingProfileFor, type WritingProfile } from "./writingProfiles";

/** Whether the composer offers Draft Review, from AI settings. */
export type DraftReviewAvailability = {
  /** Draft Assist is on and an AI provider is chosen. */
  enabled: boolean;
  /** An API key is also configured. */
  available: boolean;
  onOpenSettings?(): void;
};

/**
 * Feedback on a draft from AI, opened from the composer's Review button below
 * the message body, so the draft stays in view and editable while it is
 * reviewed. Hidden unless Draft Assist is on with a provider chosen.
 */
export function DraftReviewSection({ actions, account, enabled, available, onOpenSettings, onClose }: {
  actions: DraftReviewActions;
  /** The sending account, to say up front whether it has a writing profile. */
  account?: string;
  /** Draft Assist is on and an AI provider is chosen. */
  enabled: boolean;
  /** An API key is also configured. */
  available: boolean;
  /** Opens AI settings; without it, setup is described but not linked. */
  onOpenSettings?(): void;
  onClose?(): void;
}) {
  const [goal, setGoal] = useState("");
  const profiles = useWritingProfiles();
  const [original, setOriginal] = useState<DraftReviewSnapshot | null>(null);
  // The profile shown in the preview, so the request sends exactly what was shown.
  const [profile, setProfile] = useState<WritingProfile | null>(null);
  const [review, setReview] = useState<DraftReviewResult | null>(null);
  const [applied, setApplied] = useState<DraftReviewSnapshot | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const inFlight = useRef(false);
  const mounted = useRef(true);
  const section = useRef<HTMLElement>(null);
  const replacesBody = Boolean(original && review && original.body !== review.revisedBody);
  const protectedBody = Boolean(original?.hasInlineImages || original?.hasInlineQuotes);
  useEffect(() => {
    mounted.current = true;
    // Opened below a long draft, it scrolls into view.
    section.current?.scrollIntoView?.({ block: "nearest" });
    return () => { mounted.current = false; };
  }, []);
  // Focus goes to its first control, again once the API key check settles
  // and the setup note gives way to the review controls.
  useEffect(() => {
    section.current?.querySelector<HTMLElement>("input, button:not(:disabled):not([aria-label='Close Draft Review'])")?.focus({ preventScroll: true });
  }, [available]);

  function prepare() {
    try {
      const snapshot = actions.readDraft();
      if (!snapshot.body.trim()) throw new Error("Write some email text before requesting a review.");
      setOriginal(snapshot);
      setProfile(writingProfileFor(profiles, snapshot.account));
      setReview(null);
      setApplied(null);
      setError("");
    } catch (reason) { setError(errorMessage(reason)); }
  }

  async function getFeedback() {
    if (!original || !profile || inFlight.current) return;
    inFlight.current = true;
    setBusy(true);
    setError("");
    try {
      const current = actions.readDraft();
      if (current.fingerprint !== original.fingerprint) throw new Error("Your draft changed. Refresh the preview before requesting feedback.");
      const { provider, model, endpoint } = readAiRequestConfig("reviewing a draft", "draftReview");
      const result = await mailClient.reviewDraft({
        subject: original.subject, body: original.body, goal,
        aboutYou: profile.aboutYou, reviewInstructions: profile.reviewInstructions,
      }, provider, model, endpoint);
      if (mounted.current) setReview(result);
    } catch (reason) {
      if (mounted.current) setError(errorMessage(reason));
    } finally {
      inFlight.current = false;
      if (mounted.current) setBusy(false);
    }
  }

  function apply() {
    if (!original || !review) return;
    try {
      setApplied(actions.replaceDraft(original, { subject: review.revisedSubject, body: review.revisedBody }));
      setError("");
    } catch (reason) { setError(errorMessage(reason)); }
  }

  function undo() {
    if (!original || !applied) return;
    try {
      const restored = actions.replaceDraft(applied, { subject: original.subject, bodyHtml: original.bodyHtml });
      setOriginal(restored);
      setApplied(null);
      setError("");
    } catch (reason) { setError(errorMessage(reason)); }
  }

  if (!enabled) return null;
  const sendsProfile = Boolean(profile && hasWritingProfile(profile));
  // Before the preview, the note describes the account the draft is written from.
  const accountHasProfile = original ? sendsProfile
    : account ? hasWritingProfile(writingProfileFor(profiles, account)) : null;
  return <section ref={section} className="draft-review" aria-labelledby="draft-review-title">
    <ContextSectionHeader title="Draft Review" titleId="draft-review-title" actions={onClose ? (
      <button type="button" className="btn-icon btn-icon-sm" aria-label="Close Draft Review" onClick={onClose}><X size={ICON_SIZE.sm} /></button>
    ) : undefined} />
    {available ? <div className="draft-review-content">
      <p className="context-section-note">Get specific suggestions while keeping your voice.</p>
      <label className="draft-review-goal">
        <span>Goal for This Email <small>(optional)</small></span>
        <input value={goal} disabled={busy} placeholder="e.g. Get an introductory call" onChange={(event) => setGoal(event.target.value)} />
      </label>
      {accountHasProfile === false ? <p className="context-section-note">
        No writing profile for this account. Tell Draft Review who you are and how to review once, in AI settings.
        {onOpenSettings ? <> <button type="button" className="btn-link" onClick={onOpenSettings}>Add Writing Profile</button></> : null}
      </p> : null}
      <button type="button" className="btn btn-sm" disabled={busy} onClick={prepare}>
        {review ? "Review Again" : original ? "Refresh Preview" : "Review Draft"}
      </button>
      {original ? <>
        <details className="draft-review-preview" open={!review}>
          <summary>{review ? "Original email" : `Email content sent to ${readAiProvider()}`}</summary>
          <p><strong>Subject:</strong> {original.subject || "(no subject)"}</p>
          <pre>{original.body}</pre>
          {sendsProfile && profile ? <div className="draft-review-profile">
            <h4>Your writing profile</h4>
            {profile.aboutYou.trim() ? <p><strong>About You:</strong> {profile.aboutYou}</p> : null}
            {profile.reviewInstructions.trim() ? <p><strong>Review Instructions:</strong> {profile.reviewInstructions}</p> : null}
          </div> : null}
          {!review ? <p className="context-section-note">{sendsProfile
            ? "Only this subject, the text shown here, your writing profile, and the optional goal are sent."
            : "Only this subject, the text shown here, and the optional goal are sent."} Recipient fields, separately stored quoted history, and attachments are excluded.</p> : null}
        </details>
        {!review ? <button type="button" className="btn btn-sm btn-primary" disabled={busy} onClick={() => void getFeedback()}>
          <Sparkles size={ICON_SIZE.sm} />{busy ? "Reviewing…" : "Get Feedback"}
        </button> : <>
          <p className="draft-review-assessment" role="status">{review.assessment}</p>
          {review.suggestions.length ? <ol className="draft-review-suggestions">
            {review.suggestions.map((suggestion, index) => <li key={index}>
              <strong>{suggestion.title}</strong>
              <small>{suggestion.field === "subject" ? "Subject" : "Body"}</small>
              {suggestion.excerpt ? <blockquote>{suggestion.excerpt}</blockquote> : null}
              <p>{suggestion.reason}</p>
              {suggestion.replacement ? <p className="draft-review-wording">Try: {suggestion.replacement}</p> : null}
            </li>)}
          </ol> : <p className="context-section-note">No changes suggested.</p>}
          {reviewHasRevision(review, original) ? <div className="draft-review-preview">
            <h4>Revised email</h4>
            <p><strong>Subject:</strong> {review.revisedSubject || "(no subject)"}</p>
            <pre>{review.revisedBody}</pre>
            {applied ? <>
              <p role="status">Revision applied.</p>
              <button type="button" className="btn btn-sm" onClick={undo}>Undo Revision</button>
            </> : <>
              <p className="context-section-note">{replacesBody && protectedBody
                ? "Apply body suggestions manually to keep inline images and quoted text in place."
                : replacesBody ? "Applying replaces your written text and its formatting. Undo restores the original. Quoted history and attachments stay in place."
                  : "Only the subject changes. Your written text and formatting stay in place."}</p>
              <button type="button" className="btn btn-sm btn-primary" disabled={replacesBody && protectedBody} onClick={apply}>Apply Revision</button>
            </>}
          </div> : null}
        </>}
      </> : null}
      {error ? <p className="context-status" role="alert">{error}</p> : null}
    </div> : <div className="draft-review-content">
      <p className="context-section-note">Set up an AI provider and API key in AI settings to get writing feedback.</p>
      {onOpenSettings ? <button type="button" className="btn btn-sm" onClick={onOpenSettings}>AI Settings</button> : null}
    </div>}
  </section>;
}
