import { useEffect, useRef, useState } from "react";
import { Sparkles } from "lucide-react";
import { ContextSectionHeader } from "./ContextSections";
import { mailClient } from "./data/client";
import { readAiProvider, readAiRequestConfig } from "./aiSettings";
import type { DraftReviewResult } from "./domain";
import { reviewHasRevision, type DraftReviewActions, type DraftReviewSnapshot } from "./draftReview";
import { errorMessage } from "./errors";
import { ICON_SIZE } from "./iconSizes";

export function DraftReviewSection({ actions, available, onOpenSettings }: {
  actions: DraftReviewActions;
  available: boolean;
  onOpenSettings(): void;
}) {
  const [goal, setGoal] = useState("");
  const [original, setOriginal] = useState<DraftReviewSnapshot | null>(null);
  const [review, setReview] = useState<DraftReviewResult | null>(null);
  const [applied, setApplied] = useState<DraftReviewSnapshot | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const inFlight = useRef(false);
  const mounted = useRef(true);
  const replacesBody = Boolean(original && review && original.body !== review.revisedBody);
  const protectedBody = Boolean(original?.hasInlineImages || original?.hasInlineQuotes);
  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; };
  }, []);

  function prepare() {
    try {
      const snapshot = actions.readDraft();
      if (!snapshot.body.trim()) throw new Error("Write some email text before requesting a review.");
      setOriginal(snapshot);
      setReview(null);
      setApplied(null);
      setError("");
    } catch (reason) { setError(errorMessage(reason)); }
  }

  async function getFeedback() {
    if (!original || inFlight.current) return;
    inFlight.current = true;
    setBusy(true);
    setError("");
    try {
      const current = actions.readDraft();
      if (current.fingerprint !== original.fingerprint) throw new Error("Your draft changed. Refresh the preview before requesting feedback.");
      const { provider, model, endpoint } = readAiRequestConfig("reviewing a draft", "draftReview");
      const result = await mailClient.reviewDraft({ subject: original.subject, body: original.body, goal }, provider, model, endpoint);
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

  return <section className="context-section draft-review" aria-labelledby="draft-review-title">
    <ContextSectionHeader title="Draft review" titleId="draft-review-title" icon={<Sparkles size={ICON_SIZE.xs} />} />
    {available ? <div className="draft-review-content">
      <p className="context-section-note">Get specific suggestions while keeping your voice.</p>
      <label className="draft-review-goal">
        <span>What do you want this email to achieve? <small>(optional)</small></span>
        <input value={goal} disabled={busy} placeholder="e.g. Get an introductory call" onChange={(event) => setGoal(event.target.value)} />
      </label>
      <button type="button" className="btn btn-sm" disabled={busy} onClick={prepare}>
        {review ? "Review Again" : original ? "Refresh Preview" : "Review Draft"}
      </button>
      {original ? <>
        <details className="draft-review-preview" open={!review}>
          <summary>{review ? "Original email" : `Email content sent to ${readAiProvider()}`}</summary>
          <p><strong>Subject:</strong> {original.subject || "(no subject)"}</p>
          <pre>{original.body}</pre>
          {!review ? <p className="context-section-note">Only this subject, the text shown here, and the optional goal are sent. Recipient fields, separately stored quoted history, and attachments are excluded.</p> : null}
        </details>
        {!review ? <button type="button" className="btn btn-sm btn-primary" disabled={busy} onClick={() => void getFeedback()}>
          {busy ? "Reviewing…" : "Get Feedback"}
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
      <p className="context-section-note">Enable Draft Assist and configure an AI provider to get writing feedback.</p>
      <button type="button" className="btn btn-sm" onClick={onOpenSettings}>AI Settings</button>
    </div>}
  </section>;
}
