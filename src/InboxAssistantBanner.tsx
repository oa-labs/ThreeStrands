import { Bot } from "lucide-react";
import type { InboxAgentProposal } from "./domain";
import { ICON_SIZE } from "./iconSizes";

const KIND_LABELS: Record<InboxAgentProposal["kind"], string> = {
  archive_with_summary: "Stale Read Summary",
  archive_as_resolved: "Already Handled",
  archive_as_expired: "Expired",
  draft_reply: "Suggested Reply",
};

export function InboxAssistantBanner({
  proposals,
  busy,
  onArchive,
  onSaveToNoteAndArchive,
  onReviewDraft,
  onOpenEvidenceThread,
  onOpenEvidenceTask,
  onDismiss,
}: {
  proposals: InboxAgentProposal[];
  busy: boolean;
  onArchive(proposal: InboxAgentProposal): void;
  onSaveToNoteAndArchive(proposal: InboxAgentProposal): void;
  onReviewDraft(proposal: InboxAgentProposal, text: string): void;
  onOpenEvidenceThread(threadId: string): void;
  onOpenEvidenceTask(taskId: string): void;
  onDismiss(proposal: InboxAgentProposal): void;
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
        return (
          <div className="assistant-context-card" key={proposal.id}>
            <div className="assistant-card-meta">
              <Bot size={ICON_SIZE.xs} aria-hidden="true" />
              <span>{KIND_LABELS[proposal.kind]}</span>
            </div>
            <strong className="assistant-card-headline">{proposal.headline}</strong>
            <p className="assistant-card-reason">{proposal.reason}</p>
            {proposal.payload.summary ? (
              <div className="assistant-card-preview">{proposal.payload.summary}</div>
            ) : null}
            {proposal.payload.replyDraft ? (
              <div className="assistant-card-preview">{proposal.payload.replyDraft}</div>
            ) : null}
            <div className="assistant-card-actions">
              {proposal.kind === "archive_with_summary" ? (
                <>
                  <button type="button" className="btn btn-sm btn-primary" disabled={busy} onClick={() => onArchive(proposal)}>
                    Archive
                  </button>
                  <button type="button" className="btn btn-sm" disabled={busy} onClick={() => onSaveToNoteAndArchive(proposal)}>
                    Save to Notes &amp; Archive
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
                    <button type="button" className="btn btn-sm" onClick={() => onOpenEvidenceTask(linkedTask.id)}>
                      View Completed Task
                    </button>
                  ) : null}
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
              <button type="button" className="btn btn-sm" disabled={busy} onClick={() => onDismiss(proposal)}>
                Dismiss
              </button>
            </div>
          </div>
        );
      })}
    </section>
  );
}
