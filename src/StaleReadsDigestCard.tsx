import type { InboxAgentProposal } from "./domain";
import { digestGist, formatAgeDays, formatProposalSender } from "./inboxAssistantCard";

export const DIGEST_ENTRY_ID = "stale-reads-digest";

/** Digest items oldest first, so the stalest reads lead the list. */
export function sortDigestItems(items: readonly InboxAgentProposal[]): InboxAgentProposal[] {
  return [...items].sort(
    (a, b) => Date.parse(a.threadLastReceivedAt) - Date.parse(b.threadLastReceivedAt) || a.id.localeCompare(b.id),
  );
}

export type DigestHandlers = {
  onArchiveAll(items: InboxAgentProposal[]): void;
  onDismissAll(items: InboxAgentProposal[]): void;
  onDismiss(item: InboxAgentProposal): void;
  onSaveToNoteAndArchive(item: InboxAgentProposal): void;
  onOpenThread(threadId: string): void;
};

/** One card for every pending Stale Reads Digest item in view. */
export function StaleReadsDigestCard({
  items,
  selected,
  busy,
  showAccount,
  handlers,
  onSelect,
}: {
  items: InboxAgentProposal[];
  selected: boolean;
  busy: boolean;
  showAccount: boolean;
  handlers: DigestHandlers;
  onSelect(): void;
}) {
  const rows = sortDigestItems(items);
  const count = rows.length;
  return (
    <article
      className={`assistant-proposal-card assistant-digest-card ${selected ? "selected" : ""}`}
      aria-label={`Digest of ${count} unread ${count === 1 ? "read" : "reads"}`}
      onClick={onSelect}
    >
      <header className="assistant-proposal-header">
        <h3 className="assistant-digest-title">
          Stale Reads Digest <span aria-hidden="true">·</span> {count} unread {count === 1 ? "read" : "reads"}
        </h3>
      </header>
      <ul className="assistant-digest-rows">
        {rows.map((item) => {
          const sender = formatProposalSender(item.threadSender);
          const ageLabel = formatAgeDays(item.threadLastReceivedAt);
          const deadline = item.payload.deadline?.trim();
          const subject = item.threadSubject || item.headline || "(No subject)";
          return (
            <li key={item.id} className="assistant-digest-row" aria-label={subject}>
              <div className="assistant-proposal-header">
                <span className="assistant-sender-label" title={sender.title}>{sender.label}</span>
                {ageLabel ? (
                  <>
                    <span className="assistant-meta-sep" aria-hidden="true">·</span>
                    <span className="assistant-age-label">{ageLabel}</span>
                  </>
                ) : null}
                {showAccount ? (
                  <>
                    <span className="assistant-meta-sep" aria-hidden="true">·</span>
                    <span className="assistant-account-label">{item.accountId}</span>
                  </>
                ) : null}
                {deadline ? <span className="assistant-deadline-badge">Deadline: {deadline}</span> : null}
              </div>
              <strong className="assistant-digest-subject">{subject}</strong>
              <p className="assistant-card-reason">{digestGist(item)}</p>
              <div className="assistant-card-actions">
                <button
                  type="button"
                  className={`btn btn-sm ${item.payload.suggestSaveToNote === true ? "assistant-digest-save-suggested" : ""}`}
                  disabled={busy}
                  onClick={() => handlers.onSaveToNoteAndArchive(item)}
                >
                  Save to Notes &amp; Archive
                </button>
                <button type="button" className="btn btn-sm" onClick={() => handlers.onOpenThread(item.threadId)}>
                  Open Conversation
                </button>
                <button
                  type="button"
                  className="btn-link assistant-dismiss-btn"
                  disabled={busy}
                  onClick={() => handlers.onDismiss(item)}
                >
                  Dismiss
                </button>
              </div>
            </li>
          );
        })}
      </ul>
      <div className="assistant-card-actions assistant-digest-footer">
        <button type="button" className="btn btn-sm btn-primary" disabled={busy} onClick={() => handlers.onArchiveAll(rows)}>
          Archive All {count}
        </button>
        <button type="button" className="btn-link assistant-dismiss-btn" disabled={busy} onClick={() => handlers.onDismissAll(rows)}>
          Dismiss All
        </button>
      </div>
    </article>
  );
}
