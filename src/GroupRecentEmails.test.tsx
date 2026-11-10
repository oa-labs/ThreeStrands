import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { GroupRecentEmails } from "./GroupRecentEmails";
import { mailClient } from "./data/client";
import type { ContactTimelineItem } from "./domain";

vi.mock("./data/client", () => ({ mailClient: { contactGroupTimeline: vi.fn() } }));
const email = (index: number): ContactTimelineItem => ({ threadId: `thread-${index}`, accountId: "work", contactEmail: "member@example.com", subject: `Conversation ${index}`, snippet: "Email excerpt", sentAt: "2026-10-01T12:00:00Z", labels: [] });

describe("group recent emails", () => {
  beforeEach(() => { localStorage.clear(); vi.mocked(mailClient.contactGroupTimeline).mockReset(); });
  afterEach(cleanup);

  it("opens conversations, expands the initial rows, and loads older history once", async () => {
    const onOpenThread = vi.fn();
    vi.mocked(mailClient.contactGroupTimeline).mockResolvedValueOnce(Array.from({ length: 20 }, (_, i) => email(i))).mockResolvedValueOnce([email(20)]);
    render(<GroupRecentEmails groupId="board" accountId="work" onOpenThread={onOpenThread}/>);
    fireEvent.click(await screen.findByRole("button", { name: /Conversation 0/ }));
    expect(onOpenThread).toHaveBeenCalledWith("thread-0");
    expect(mailClient.contactGroupTimeline).toHaveBeenCalledWith("board", 0, 20, "work");
    expect(screen.queryByRole("button", { name: /Conversation 19/ })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Show 12 more" }));
    expect(screen.getByRole("button", { name: /Conversation 19/ })).toBeVisible();
    fireEvent.click(screen.getByRole("button", { name: "Load older emails" }));
    expect(await screen.findByRole("button", { name: /Conversation 20/ })).toBeVisible();
    expect(mailClient.contactGroupTimeline).toHaveBeenLastCalledWith("board", 20, 20, "work");
    expect(screen.queryByRole("button", { name: "Load older emails" })).not.toBeInTheDocument();
  });

  it("retains loaded history after a page fails and retries the same offset", async () => {
    vi.mocked(mailClient.contactGroupTimeline).mockResolvedValueOnce(Array.from({ length: 20 }, (_, i) => email(i))).mockRejectedValueOnce(new Error("History unavailable")).mockResolvedValueOnce([email(20)]);
    render(<GroupRecentEmails groupId="board" accountId={null} onOpenThread={vi.fn()}/>);
    fireEvent.click(await screen.findByRole("button", { name: "Show 12 more" }));
    fireEvent.click(screen.getByRole("button", { name: "Load older emails" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("History unavailable");
    expect(screen.getByRole("button", { name: /Conversation 0/ })).toBeVisible();
    fireEvent.click(screen.getByRole("button", { name: "Try Again" }));
    expect(await screen.findByRole("button", { name: /Conversation 20/ })).toBeVisible();
    expect(mailClient.contactGroupTimeline).toHaveBeenLastCalledWith("board", 20, 20, undefined);
  });

  it("ignores late responses after switching groups and shows empty history", async () => {
    let finish!: (items: ContactTimelineItem[]) => void;
    vi.mocked(mailClient.contactGroupTimeline).mockImplementationOnce(() => new Promise(resolve => { finish = resolve; })).mockResolvedValueOnce([]);
    const { rerender } = render(<GroupRecentEmails key="board" groupId="board" accountId={null} onOpenThread={vi.fn()}/>);
    rerender(<GroupRecentEmails key="family" groupId="family" accountId={null} onOpenThread={vi.fn()}/>);
    expect(await screen.findByText("No emails with current group members.")).toBeVisible();
    finish([email(0)]);
    await waitFor(() => expect(screen.queryByRole("button", { name: /Conversation 0/ })).not.toBeInTheDocument());
  });
});
