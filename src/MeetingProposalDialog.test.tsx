import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { MeetingProposal } from "./domain";
import { MeetingProposalDialog } from "./MeetingProposalDialog";

const proposal: MeetingProposal = {
  type: "meeting", intent: "Plan the launch", title: "Launch sync", participants: ["jane@example.com"], location: null,
  rawTimeLanguage: "next Tuesday", normalizedStart: null, normalizedEnd: null, searchRangeStart: null, searchRangeEnd: null,
  durationMinutes: 30, timeZone: null, confidence: 0.9, evidence: { sourceMessageId: "message-1", excerpt: "Can we meet Tuesday?" },
};

describe("MeetingProposalDialog", () => {
  afterEach(cleanup);

  it("edits participants as badges and saves the list only on submit", () => {
    const onSave = vi.fn();
    render(<MeetingProposalDialog proposal={proposal} onClose={vi.fn()} onSave={onSave} />);
    const dialog = screen.getByRole("dialog", { name: "Edit Meeting Proposal" });
    expect(within(dialog).getByRole("button", { name: "Remove jane@example.com" })).toBeInTheDocument();

    const input = within(dialog).getByRole("textbox", { name: "Participants" });
    fireEvent.paste(input, { clipboardData: { getData: () => "bob@example.com; ada@example.com" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Remove jane@example.com" }));
    expect(onSave).not.toHaveBeenCalled();

    fireEvent.click(within(dialog).getByRole("button", { name: "Save Proposal" }));
    expect(onSave).toHaveBeenCalledWith(expect.objectContaining({ participants: ["bob@example.com", "ada@example.com"] }));
  });
});
