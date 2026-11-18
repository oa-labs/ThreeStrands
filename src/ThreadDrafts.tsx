import type { DraftSummary } from "./correspondence";
import type { Thread } from "./domain";

/** The unsent drafts that belong to `thread`, wherever they were written. */
export function threadDrafts<T extends Pick<DraftSummary, "account" | "threadId">>(drafts: T[], thread: Pick<Thread, "accountId" | "providerThreadId">): T[] {
  return drafts.filter((draft) => draft.account === thread.accountId && draft.threadId === thread.providerThreadId);
}

/** Shows each unsent reply at the end of its conversation, as a draft rather than a message. */
export function ThreadDrafts({ drafts, onOpen }: { drafts: DraftSummary[]; onOpen(draft: DraftSummary): void }) {
  if (!drafts.length) return null;
  return <>{drafts.map((draft) => (
    <div key={draft.id} className="notice thread-draft" role="group" aria-label="Unsent draft">
      <span><strong>Draft</strong> {draft.to ? `to ${draft.to}` : "with no recipients"}</span>
      <button type="button" className="btn btn-sm" onClick={() => onOpen(draft)}>Resume Draft</button>
    </div>
  ))}</>;
}
