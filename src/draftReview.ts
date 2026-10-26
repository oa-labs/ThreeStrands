import type { DraftReviewResult } from "./domain";

/** A synchronous capture of the actual editor DOM, including unsaved edits. */
export type DraftReviewSnapshot = {
  id: string;
  /** The sending account, whose writing profile the review uses. */
  account: string;
  subject: string;
  body: string;
  bodyHtml: string;
  fingerprint: string;
  hasInlineImages: boolean;
  hasInlineQuotes: boolean;
};

export type DraftReviewEdit = { subject: string } & ({ body: string } | { bodyHtml: string });

export type DraftReviewActions = {
  readDraft(): DraftReviewSnapshot;
  replaceDraft(expected: DraftReviewSnapshot, edit: DraftReviewEdit): DraftReviewSnapshot;
};

export function reviewHasRevision(review: DraftReviewResult, original: DraftReviewSnapshot): boolean {
  return review.revisedSubject !== original.subject || review.revisedBody !== original.body;
}
