import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { InboxAgentProposal } from "./domain";
import { isSafeForBatchArchive } from "./inboxAssistantCard";
import { InboxAssistantBanner } from "./InboxAssistantBanner";
import type { SenderHygieneHandlers } from "./SenderHygieneActions";

function proposal(payload: InboxAgentProposal["payload"]): InboxAgentProposal {
  return {
    id: "h1",
    agentId: "sender_hygiene",
    accountId: "you@example.com",
    threadId: "t1",
    threadRevision: "2026-11-20T00:00:00Z",
    threadSubject: "Weekend deals",
    threadSender: "Acme Deals <deals@acme.example>",
    threadLastReceivedAt: "2026-11-20T00:00:00Z",
    threadSnippet: "Save big",
    threadUnread: true,
    threadStarred: false,
    kind: "sender_hygiene",
    state: "pending",
    headline: "You usually skip Acme Deals",
    reason: "4 unread emails older than 14 days.",
    payload,
    evidence: [],
    createdAt: 1,
    updatedAt: 1,
  };
}

function render_(p: InboxAgentProposal, handlers: SenderHygieneHandlers) {
  render(
    <InboxAssistantBanner
      proposals={[p]}
      busy={false}
      onArchive={vi.fn()}
      onSaveToNoteAndArchive={vi.fn()}
      onCreateTask={vi.fn()}
      onReopenTask={vi.fn()}
      onReviewDraft={vi.fn()}
      onOpenEvidenceThread={vi.fn()}
      onOpenEvidenceTask={vi.fn()}
      onDismiss={vi.fn()}
      senderHygiene={handlers}
    />,
  );
}

describe("InboxAssistantBanner Sender Hygiene", () => {
  afterEach(cleanup);

  it("offers the sender actions for the conversation and calls their handlers", () => {
    const handlers: SenderHygieneHandlers = {
      splits: [],
      onUnsubscribe: vi.fn(),
      onMute: vi.fn(),
      onNewSplit: vi.fn(),
      onArchiveAll: vi.fn(),
    };
    const p = proposal({ senderEmail: "deals@acme.example", recommendation: "unsubscribe", messageId: "m1", inboxThreadIds: ["t1", "t2"] });
    render_(p, handlers);
    expect(screen.getByText("Ignored Sender")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Unsubscribe" }));
    fireEvent.click(screen.getByRole("button", { name: "Mute…" }));
    fireEvent.click(screen.getByRole("button", { name: "New Split…" }));
    fireEvent.click(screen.getByRole("button", { name: "Archive 2 in Inbox" }));
    expect(handlers.onUnsubscribe).toHaveBeenCalledWith(p);
    expect(handlers.onMute).toHaveBeenCalledWith(p);
    expect(handlers.onNewSplit).toHaveBeenCalledWith(p);
    expect(handlers.onArchiveAll).toHaveBeenCalledWith(p);
  });

  it("is never safe for one-click batch archive", () => {
    expect(isSafeForBatchArchive(proposal({ recommendation: "mute" }))).toBe(false);
  });
});
