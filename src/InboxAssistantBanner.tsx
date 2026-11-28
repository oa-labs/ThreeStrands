import { Bot } from "lucide-react";
import type { InboxAgentProposal } from "./domain";
import { ICON_SIZE } from "./iconSizes";
import { ASSISTANT_KIND_LABELS, digestGist, proposalTakeaways } from "./inboxAssistantCard";
import { SenderHygieneActions, type SenderHygieneHandlers } from "./SenderHygieneActions";

export function InboxAssistantBanner({
  proposals,
  busy,
  onArchive,
  onSaveToNoteAndArchive,
  onCreateTask,
  onReopenTask,
  onReviewDraft,
  onOpenEvidenceThread,
  onOpenEvidenceTask,
  onDismiss,
  senderHygiene,
}: {
  proposals: InboxAgentProposal[];
  busy: boolean;
  onArchive(proposal: InboxAgentProposal): void;
  onSaveToNoteAndArchive(proposal: InboxAgentProposal): void;
  onCreateTask(proposal: InboxAgentProposal): void;
  onReopenTask(proposal: InboxAgentProposal, taskId: string): void;
  onReviewDraft(proposal: InboxAgentProposal, text: string): void;
  onOpenEvidenceThread(threadId: string): void;
  onOpenEvidenceTask(taskId: string): void;
  onDismiss(proposal: InboxAgentProposal): void;
  senderHygiene: SenderHygieneHandlers;
}) {
  if (proposals.length === 0) return null;

  return (
    <section className="assistant-context-section" aria-label="Inbox Assistant Suggestions">
      <div className="assistant-context-header">
        <span className="eyebrow">Inbox Assistant</span>
        <span className="assistant-count-badge">{proposals.length}</span>
      </div>
      {proposals.map((proposal) => {
        const linkedThread = proposal.evidence.find((e) => e.kind === "thread");
        const linkedTask = proposal.evidence.find((e) => e.kind === "task");
        const isSummary = proposal.kind === "archive_with_summary";
        const takeaways = isSummary ? proposalTakeaways(proposal) : [];
        const deadline = proposal.payload.deadline?.trim();
        return (
          <div className="assistant-context-card" key={proposal.id}>
            <div className="assistant-card-meta">
              <Bot size={ICON_SIZE.xs} aria-hidden="true" />
              <span>{ASSISTANT_KIND_LABELS[proposal.kind]}</span>
              {deadline ? <span className="assistant-deadline-badge">Deadline: {deadline}</span> : null}
            </div>
            <strong className="assistant-card-headline">{proposal.headline}</strong>
            {isSummary ? (
              takeaways.length > 0 ? (
                <ul className="assistant-takeaways">
                  {takeaways.map((item, idx) => (
                    <li key={idx}>{item}</li>
                  ))}
                </ul>
              ) : null
            ) : proposal.kind === "digest_item" ? (
              <p className="assistant-card-reason">{digestGist(proposal)}</p>
            ) : (
              <p className="assistant-card-reason">{proposal.reason}</p>
            )}
            {proposal.payload.replyDraft ? (
              <div className="assistant-card-preview">{proposal.payload.replyDraft}</div>
            ) : null}
            <div className="assistant-card-actions">
              {proposal.kind === "archive_with_summary" ? (
                <>
                  <button type="button" className="btn btn-sm btn-primary" disabled={busy} onClick={() => onArchive(proposal)}>
                    Archive
                  </button>
                  {proposal.payload.suggestSaveToNote === true ? (
                    <button type="button" className="btn btn-sm" disabled={busy} onClick={() => onSaveToNoteAndArchive(proposal)}>
                      Save to Notes &amp; Archive
                    </button>
                  ) : null}
                  <button type="button" className="btn btn-sm" disabled={busy} onClick={() => onCreateTask(proposal)}>
                    Create Task
                  </button>
                </>
              ) : null}
              {(proposal.kind === "archive_as_resolved" || proposal.kind === "archive_as_expired") ? (
                <>
                  <button type="button" className="btn btn-sm btn-primary" disabled={busy} onClick={() => onArchive(proposal)}>
                    Archive
                  </button>
                  {linkedThread ? (
                    <button type="button" className="btn btn-sm" onClick={() => onOpenEvidenceThread(linkedThread.id)}>
                      View Resolving Thread
                    </button>
                  ) : null}
                  {linkedTask ? (
                    <>
                      <button type="button" className="btn btn-sm" disabled={busy} onClick={() => onReopenTask(proposal, linkedTask.id)}>
                        Reopen Task
                      </button>
                      <button type="button" className="btn btn-sm" onClick={() => onOpenEvidenceTask(linkedTask.id)}>
                        View Completed Task
                      </button>
                    </>
                  ) : (
                    <button type="button" className="btn btn-sm" disabled={busy} onClick={() => onCreateTask(proposal)}>
                      Create Task
                    </button>
                  )}
                </>
              ) : null}
              {proposal.kind === "draft_reply" && proposal.payload.replyDraft ? (
                <>
                  <button
                    type="button"
                    className="btn btn-sm btn-primary"
                    disabled={busy}
                    onClick={() => onReviewDraft(proposal, proposal.payload.replyDraft!)}
                  >
                    Review Draft
                  </button>
                  {proposal.payload.gentleOutDraft ? (
                    <button
                      type="button"
                      className="btn btn-sm"
                      disabled={busy}
                      onClick={() => onReviewDraft(proposal, proposal.payload.gentleOutDraft!)}
                    >
                      Quick Check-In
                    </button>
                  ) : null}
                </>
              ) : null}
              {proposal.kind === "digest_item" ? (
                <>
                  <button type="button" className="btn btn-sm btn-primary" disabled={busy} onClick={() => onArchive(proposal)}>
                    Archive
                  </button>
                  <button type="button" className="btn btn-sm" disabled={busy} onClick={() => onSaveToNoteAndArchive(proposal)}>
                    Save to Notes &amp; Archive
                  </button>
                </>
              ) : null}
              {proposal.kind === "sender_hygiene" ? (
                <SenderHygieneActions proposal={proposal} busy={busy} handlers={senderHygiene} />
              ) : null}
              <button type="button" className="btn-link assistant-dismiss-btn" disabled={busy} onClick={() => onDismiss(proposal)}>
                Dismiss
              </button>
            </div>
          </div>
        );
      })}
    </section>
  );
}
