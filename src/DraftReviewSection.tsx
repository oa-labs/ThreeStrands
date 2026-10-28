import { useEffect, useId, useLayoutEffect, useRef, useState } from "react";
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
  /** Opens Settings at Writing Profile. */
  onOpenWritingProfile?(): void;
};

/** What one review sent, shown afterwards under What Was Sent. */
type SentReview = { snapshot: DraftReviewSnapshot; profile: WritingProfile; goal: string };

/**
 * Feedback on a draft from AI, opened from the composer's Review button below
 * the message body, so the draft stays in view and editable while it is
 * reviewed. Opening it is the request: the review starts at once with the
 * sending account's writing profile, and a goal can be added afterwards to
 * review again. Hidden unless Draft Assist is on with a provider chosen.
 */
export function DraftReviewSection({ actions, account, goal: keptGoal, onGoalChange, enabled, available, onOpenSettings, onOpenWritingProfile, onClose, onApplied }: {
  actions: DraftReviewActions;
  /** The goal, kept by the composer so closing the panel does not clear it. */
  goal?: string;
  onGoalChange?(goal: string): void;
  /** The sending account, to say whether it has a writing profile. */
  account?: string;
  /** Draft Assist is on and an AI provider is chosen. */
  enabled: boolean;
  /** An API key is also configured. */
  available: boolean;
  /** Opens AI settings; without it, setup is described but not linked. */
  onOpenSettings?(): void;
  /** Opens Settings at Writing Profile; without it, the profile is described but not linked. */
  onOpenWritingProfile?(): void;
  onClose?(): void;
  /** Reveal the final draft after the review has collapsed. */
  onApplied?(): void;
}) {
  const [localGoal, setLocalGoal] = useState("");
  const goal = keptGoal ?? localGoal;
  const setGoal = onGoalChange ?? setLocalGoal;
  const profiles = useWritingProfiles();
  const [sent, setSent] = useState<SentReview | null>(null);
  // The draft the review applies to; Undo replaces it with the restored draft.
  const [original, setOriginal] = useState<DraftReviewSnapshot | null>(null);
  const [review, setReview] = useState<DraftReviewResult | null>(null);
  const [applied, setApplied] = useState<DraftReviewSnapshot | null>(null);
  const [expanded, setExpanded] = useState(true);
  const contentId = useId();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const inFlight = useRef(false);
  const mounted = useRef(true);
  const started = useRef(false);
  const section = useRef<HTMLElement>(null);
  const replacesBody = Boolean(original && review && original.body !== review.revisedBody);
  const protectedBody = Boolean(original?.hasInlineImages || original?.hasInlineQuotes);
  useEffect(() => {
    mounted.current = true;
    // Opened below a long draft, it scrolls into view.
    section.current?.scrollIntoView?.({ block: "start" });
    return () => { mounted.current = false; };
  }, []);
  useLayoutEffect(() => {
    if (applied) onApplied?.();
  }, [applied, onApplied]);
  // Opening the panel asks for the review, once the API key check confirms
  // one can be sent.
  useEffect(() => {
    if (!enabled || !available || started.current) return;
    started.current = true;
    void latestRequest.current();
  }, [enabled, available]);

  async function requestReview() {
    if (inFlight.current) return;
    inFlight.current = true;
    setBusy(true);
    setError("");
    try {
      const snapshot = actions.readDraft();
      if (!snapshot.body.trim()) throw new Error("Write some email text before requesting a review.");
      const profile = writingProfileFor(profiles, snapshot.account);
      const request = { snapshot, profile, goal: goal.trim() };
      setExpanded(true);
      setReview(null);
      setApplied(null);
      setSent(request);
      setOriginal(snapshot);
      const { provider, model, endpoint } = readAiRequestConfig("reviewing a draft", "draftReview");
      const result = await mailClient.reviewDraft({
        subject: snapshot.subject, body: snapshot.body, goal: request.goal,
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

  const latestRequest = useRef(requestReview);
  latestRequest.current = requestReview;

  function apply() {
    if (!original || !review) return;
    try {
      setApplied(actions.replaceDraft(original, { subject: review.revisedSubject, body: review.revisedBody }));
      setExpanded(false);
      setError("");
    } catch (reason) { setError(errorMessage(reason)); }
  }

  function undo() {
    if (!original || !applied) return;
    try {
      const restored = actions.replaceDraft(applied, { subject: original.subject, bodyHtml: original.bodyHtml });
      setOriginal(restored);
      setExpanded(true);
      setApplied(null);
      setError("");
    } catch (reason) { setError(errorMessage(reason)); }
  }

  if (!enabled) return null;
  const reviewedAccount = sent?.snapshot.account ?? account;
  const accountHasProfile = reviewedAccount ? hasWritingProfile(writingProfileFor(profiles, reviewedAccount)) : true;
  const sentProfile = sent && hasWritingProfile(sent.profile) ? sent.profile : null;
  return <section ref={section} className="draft-review" aria-labelledby="draft-review-title" aria-busy={busy}>
    <ContextSectionHeader title="Draft Review" titleId="draft-review-title" icon={<Sparkles size={ICON_SIZE.sm} />} actions={onClose ? (
      <button type="button" className="btn-icon btn-icon-sm" aria-label="Close Draft Review" onClick={onClose}><X size={ICON_SIZE.sm} /></button>
    ) : undefined} />
    {applied ? <div className="draft-review-applied">
      <span role="status">Revision applied.</span>
      <button type="button" className="btn btn-sm" onClick={undo}>Undo Revision</button>
      <button type="button" className="btn btn-sm" aria-expanded={expanded} aria-controls={contentId} onClick={() => setExpanded((value) => !value)}>{expanded ? "Hide Review" : "Show Review"}</button>
    </div> : null}
    {error ? <p className="context-status" role="alert">{error}</p> : null}
    <div id={contentId} hidden={Boolean(applied) && !expanded}>
    {available ? <div className="draft-review-content">
      {busy ? <p className="context-section-note" role="status">Reviewing…</p> : null}
      {review && original && !busy ? <>
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
          {!applied ? <>
            <p className="context-section-note">{replacesBody && protectedBody
              ? "Apply body suggestions manually to keep inline images and quoted text in place."
              : replacesBody ? "Applying replaces your written text and its formatting. Undo restores the original. Quoted history and attachments stay in place."
                : "Only the subject changes. Your written text and formatting stay in place."}</p>
            <button type="button" className="btn btn-sm btn-primary" disabled={replacesBody && protectedBody} onClick={apply}>Apply Revision</button>
          </> : null}
        </div> : null}
      </> : null}
      {!busy && (review || error) ? <div className="draft-review-again">
        <label className="draft-review-goal">
          <span>{review ? "Not quite right? Add a goal for this email" : "Goal for This Email"} <small>(optional)</small></span>
          <input value={goal} placeholder="e.g. Get an introductory call" onChange={(event) => setGoal(event.target.value)}
            onKeyDown={(event) => { if (event.key === "Enter") { event.preventDefault(); void requestReview(); } }} />
        </label>
        <button type="button" className="btn btn-sm" onClick={() => void requestReview()}>
          <Sparkles size={ICON_SIZE.sm} />Review Again
        </button>
        {!accountHasProfile ? <p className="context-section-note">
          No writing profile for this account. Tell Draft Review who you are and how to review once, in Settings.
          {onOpenWritingProfile ? <> <button type="button" className="btn-link" onClick={onOpenWritingProfile}>Add Writing Profile</button></> : null}
        </p> : null}
      </div> : null}
      {sent && !busy ? <details className="draft-review-preview">
        <summary>What was sent to {readAiProvider()}</summary>
        <p><strong>Subject:</strong> {sent.snapshot.subject || "(no subject)"}</p>
        <pre>{sent.snapshot.body}</pre>
        {sentProfile ? <div className="draft-review-profile">
          <h4>Your writing profile</h4>
          {sentProfile.aboutYou.trim() ? <p><strong>About You:</strong> {sentProfile.aboutYou}</p> : null}
          {sentProfile.reviewInstructions.trim() ? <p><strong>Review Instructions:</strong> {sentProfile.reviewInstructions}</p> : null}
        </div> : null}
        {sent.goal ? <p><strong>Goal:</strong> {sent.goal}</p> : null}
        <p className="context-section-note">Only the subject, written text{sentProfile ? ", writing profile," : ""} and goal shown here are sent. Recipient fields, separately stored quoted history, and attachments are excluded.</p>
      </details> : null}
    </div> : <div className="draft-review-content">
      <p className="context-section-note">Set up an AI provider and API key in AI settings to get writing feedback.</p>
      {onOpenSettings ? <button type="button" className="btn btn-sm" onClick={onOpenSettings}>AI Settings</button> : null}
    </div>}
    </div>
  </section>;
}
