import type { InboxAgentProposal, SplitInbox } from "./domain";
import { matchingSplit } from "./senderSignals";

/** What a Sender Hygiene suggestion can do, supplied by the app. Each one accepts the suggestion once it succeeds. */
export type SenderHygieneHandlers = {
  /** Split inboxes across accounts; one that already takes the sender's mail replaces New Split. */
  splits: SplitInbox[];
  /** Opens the conversation and its unsubscribe confirmation. */
  onUnsubscribe(proposal: InboxAgentProposal): void;
  /** Opens the Mute dialog for the sender. */
  onMute(proposal: InboxAgentProposal): void;
  /** Opens the New Split dialog for the sender. */
  onNewSplit(proposal: InboxAgentProposal): void;
  /** Archives every conversation the sender still has in the Inbox, with Undo. */
  onArchiveAll(proposal: InboxAgentProposal): void;
};

/** The sender-specific actions on a Sender Hygiene card or context-panel suggestion. */
export function SenderHygieneActions({ proposal, busy, handlers, small = true }: {
  proposal: InboxAgentProposal;
  busy: boolean;
  handlers: SenderHygieneHandlers;
  small?: boolean;
}) {
  const email = proposal.payload.senderEmail ?? "";
  const unsubscribeFirst = proposal.payload.recommendation === "unsubscribe" && Boolean(proposal.payload.messageId);
  const split = email ? matchingSplit(handlers.splits, proposal.accountId, email) : null;
  const inboxCount = proposal.payload.inboxThreadIds?.length ?? 0;
  const size = small ? "btn btn-sm" : "btn";
  return (
    <>
      {proposal.payload.messageId ? (
        <button type="button" className={unsubscribeFirst ? `${size} btn-primary` : size} disabled={busy} onClick={() => handlers.onUnsubscribe(proposal)}>
          Unsubscribe
        </button>
      ) : null}
      <button type="button" className={unsubscribeFirst ? size : `${size} btn-primary`} disabled={busy} onClick={() => handlers.onMute(proposal)}>
        Mute…
      </button>
      {split ? (
        <span className="context-sender-state">Sorted into {split.name}</span>
      ) : (
        <button type="button" className={size} disabled={busy} onClick={() => handlers.onNewSplit(proposal)}>
          New Split…
        </button>
      )}
      {inboxCount > 1 ? (
        <button type="button" className={size} disabled={busy} onClick={() => handlers.onArchiveAll(proposal)}>
          Archive {inboxCount} in Inbox
        </button>
      ) : null}
    </>
  );
}
