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

  it("preserves extracted names and header addresses without treating them as invalid invitees", () => {
    const onSave = vi.fn();
    const participants = ["Jennifer Williams", "Alex <alex@example.com>"];
    render(<MeetingProposalDialog proposal={{ ...proposal, participants }} onClose={vi.fn()} onSave={onSave} />);
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: "Participants" })).not.toHaveAttribute("aria-invalid");
    fireEvent.change(screen.getByRole("textbox", { name: "Title" }), { target: { value: "Podcast interview" } });
    fireEvent.click(screen.getByRole("button", { name: "Save Proposal" }));
    expect(onSave).toHaveBeenCalledWith(expect.objectContaining({ title: "Podcast interview", participants }));
  });

  it("adds full names with spaces and removes participants", () => {
    const onSave = vi.fn();
    render(<MeetingProposalDialog proposal={{ ...proposal, participants: ["Jennifer Williams"] }} onClose={vi.fn()} onSave={onSave} />);
    const input = screen.getByRole("textbox", { name: "Participants" });
    fireEvent.change(input, { target: { value: "Morgan Lee" } });
    fireEvent.keyDown(input, { key: "Enter" });
    expect(screen.getByRole("button", { name: "Remove Morgan Lee" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Remove Jennifer Williams" }));
    fireEvent.click(screen.getByRole("button", { name: "Save Proposal" }));
    expect(onSave).toHaveBeenCalledWith(expect.objectContaining({ participants: ["Morgan Lee"] }));
  });

  it.each(["blur", "shortcut"])("keeps a newly typed name when saving via %s", (method) => {
    const onSave = vi.fn();
    render(<MeetingProposalDialog proposal={proposal} onClose={vi.fn()} onSave={onSave} />);
    const input = screen.getByRole("textbox", { name: "Participants" });
    fireEvent.change(input, { target: { value: "Morgan Lee" } });
    if (method === "blur") {
      fireEvent.blur(input);
      fireEvent.click(screen.getByRole("button", { name: "Save Proposal" }));
    } else {
      fireEvent.keyDown(input, { key: "Enter", ctrlKey: true });
    }
    expect(onSave).toHaveBeenCalledWith(expect.objectContaining({ participants: ["jane@example.com", "Morgan Lee"] }));
  });
});
